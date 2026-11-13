//! What's done to Messages and Mailboxes: flagging, deleting, archiving, moving, labelling and undoing it, the
//! Selection and Filters, sorting, searching, unsubscribing, piping and bouncing.

use std::collections::HashMap;
use std::sync::Arc;

use muttrs_mail::Role;
use muttrs_mail::client::{self, Flag, Verdict};
use wcgw_kit::BufferId;
use wcgw_kit::shell::CursorLine;

use super::{
    Document, Effect, Effects, Io, Kit, Line, Muttrs, Opened, Span, Target, Targets, UNDOES, Undo,
};
use crate::filter::Filter;
use crate::hl;
use crate::index::{Index, Sort};
use crate::sidebar::{self, Row};
use crate::tasks;

/// What's done with Messages once they're fetched.
#[derive(Debug)]
pub(crate) enum Then {
    /// They're piped to this command.
    Pipe(String),
    /// They're sent on as they are to these (mutt's bounce).
    Bounce(Vec<muttrs_mail::Address>),
}

/// Messages being fetched, to do something with once they've all come.
#[derive(Debug)]
pub(crate) struct Piping {
    then: Then,
    account: usize,
    mailbox: String,
    /// Their UIDs, in the order they're piped: the oldest first.
    uids: Vec<u32>,
    got: HashMap<u32, Arc<Vec<u8>>>,
}

/// A Role, as people name it.
fn role_name(role: Role) -> &'static str {
    match role {
        Role::Inbox => "Inbox",
        Role::Drafts => "Drafts",
        Role::Sent => "Sent",
        Role::Archive => "Archive",
        Role::All => "All Mail",
        Role::Flagged => "Flagged",
        Role::Junk => "Junk",
        Role::Trash => "Trash",
    }
}

impl Muttrs {
    /// The Messages an action is for: the one the Pager shows; else those selected in the Index; else the one at its
    /// cursor, with its thread when that's folded into it.
    pub(crate) fn targets(&self, kit: &Kit) -> Option<Targets> {
        let current = kit.current_buffer();
        if let Some(pager) = self.pagers.get(&current) {
            let opened = pager.opened.as_ref()?;
            return Some(Targets {
                account: opened.account,
                mailbox: opened.mailbox.clone(),
                uids: vec![opened.uid],
            });
        }
        let index = self.indexes.get(&current)?;
        let selection = index.selection();
        let uids = if selection.is_empty() {
            let Some(Target::Message(opened)) = kit.target_under_cursor() else {
                return None;
            };
            Self::at_cursor(kit, index, opened.uid)
        } else {
            selection
        };
        Some(Targets {
            account: index.account,
            mailbox: index.mailbox.clone(),
            uids,
        })
    }

    /// The Message `uid` at the cursor of `index`, with its thread when that's folded into it.
    fn at_cursor(kit: &Kit, index: &Index, uid: u32) -> Vec<u32> {
        if kit.on_closed_fold() {
            index.thread_of(uid)
        } else {
            vec![uid]
        }
    }

    /// The Messages an action is for, or why there are none.
    fn targets_or_say(&self, kit: &Kit) -> Result<Targets, String> {
        self.targets(kit)
            .ok_or_else(|| "No Message here: that's for the Index or the Pager".to_owned())
    }

    /// The Index of `targets`' Mailbox, if it's open.
    fn index_for(&mut self, targets: &Targets) -> Option<(BufferId, &mut Index)> {
        self.index_of_mut(targets.account, &targets.mailbox)
    }

    /// Toggles `flag` on the Messages an action is for: on all, unless all have it already.
    pub(crate) fn toggle(&mut self, kit: &mut Kit, flag: Flag) -> Result<Effects, String> {
        let targets = self.targets_or_say(kit)?;
        if flag == Flag::Seen {
            // Marked as the user says, rather than once it's been shown a while.
            self.reading = None;
        }
        let has = |flags: &muttrs_mail::Flags| match flag {
            Flag::Seen => flags.seen,
            Flag::Flagged => flags.flagged,
            Flag::Answered => flags.answered,
        };
        let mut changed = 0;
        let mut on = true;
        if let Some((buffer, index)) = self.index_for(&targets) {
            on = !targets
                .uids
                .iter()
                .filter_map(|&uid| index.find(uid))
                .all(|envelope| has(&envelope.flags));
            for &uid in &targets.uids {
                let Some(envelope) = index.find_mut(uid) else {
                    continue;
                };
                if has(&envelope.flags) != on {
                    changed += 1;
                }
                tasks::set_flag(&mut envelope.flags, flag, on);
            }
            self.dirty.insert(buffer);
        }
        if let Some(state) = self.accounts.get_mut(targets.account) {
            if flag == Flag::Seen
                && let Some(counts) = state.counts.get_mut(&targets.mailbox)
            {
                counts.unread = if on {
                    counts.unread.saturating_sub(changed)
                } else {
                    counts.unread + changed
                };
            }
            state.ask(client::Request::Flag {
                mailbox: targets.mailbox,
                uids: targets.uids,
                flag,
                on,
            });
        }
        Ok(Vec::new())
    }

    /// Deletes (to Trash) or archives the Messages an action is for, going on to the next one.
    pub(crate) fn remove(&mut self, kit: &mut Kit, archive: bool) -> Result<Effects, String> {
        let targets = self.targets_or_say(kit)?;
        // Deleted from Trash, they're deleted for good: there's no undoing that.
        let in_trash = self.accounts.get(targets.account).is_some_and(|state| {
            state
                .mailbox(&targets.mailbox)
                .is_some_and(|known| known.role == Some(Role::Trash))
        });
        if !in_trash {
            let went = if archive {
                client::Went::Archived
            } else {
                client::Went::Trash
            };
            self.remember_undo(&targets, went);
        }
        // The next Message to read asked first, so that it doesn't wait for them to go.
        let effects = self.taken_out(kit, &targets);
        let Some(state) = self.accounts.get_mut(targets.account) else {
            return Ok(effects);
        };
        let Targets { mailbox, uids, .. } = targets;
        // The worker knows where to: the Mailbox with the Role, by the server or the configuration. Without one it
        // says so, and lists the Mailbox again, which brings the Messages back.
        state.ask(if archive {
            client::Request::Archive { mailbox, uids }
        } else {
            client::Request::Delete { mailbox, uids }
        });
        Ok(effects)
    }

    /// Tells the server whether the Messages an action is for are spam, as its filter learns, going on to the next
    /// one: spam, they're moved to Junk, marked `$Junk`; not, those in Junk are moved to the Inbox, marked `$NotJunk`.
    pub(crate) fn judge(&mut self, kit: &mut Kit, verdict: Verdict) -> Result<Effects, String> {
        let targets = self.targets_or_say(kit)?;
        let in_junk = self.accounts.get(targets.account).is_some_and(|state| {
            state
                .mailbox(&targets.mailbox)
                .is_some_and(|known| known.role == Some(Role::Junk))
        });
        match verdict {
            Verdict::Junk if in_junk => return Err("They're in Junk already".to_owned()),
            Verdict::NotJunk if !in_junk => {
                return Err(":notjunk is for Messages in Junk".to_owned());
            }
            Verdict::Junk | Verdict::NotJunk => {}
        }
        self.remember_undo(&targets, client::Went::Judged(verdict));
        let effects = self.taken_out(kit, &targets);
        if let Some(state) = self.accounts.get_mut(targets.account) {
            state.ask(client::Request::Judge {
                mailbox: targets.mailbox,
                uids: targets.uids,
                verdict,
            });
        }
        Ok(effects)
    }

    /// Keeps, for `u` to undo, that `targets` went elsewhere: those with a `Message-ID`, which find them there.
    fn remember_undo(&mut self, targets: &Targets, went: client::Went) {
        let message_ids: Vec<String> = self
            .mailboxes
            .get(&(targets.account, targets.mailbox.clone()))
            .and_then(|buffer| self.indexes.get(buffer))
            .into_iter()
            .flat_map(|index| targets.uids.iter().filter_map(|&uid| index.find(uid)))
            .filter_map(|envelope| envelope.message_id.clone())
            .collect();
        if message_ids.is_empty() {
            return;
        }
        if self.undo.len() == UNDOES {
            self.undo.remove(0);
        }
        self.undo.push(Undo {
            account: targets.account,
            went,
            from: targets.mailbox.clone(),
            message_ids,
        });
    }

    /// Moves the Messages last moved, archived or deleted to Trash back where they were.
    pub(crate) fn undo(&mut self, kit: &mut Kit) {
        let Some(undo) = self.undo.pop() else {
            kit.info("Nothing to undo");
            return;
        };
        let Some(state) = self.accounts.get_mut(undo.account) else {
            return;
        };
        let task = state.ask(client::Request::MoveBack {
            went: undo.went.clone(),
            to: undo.from.clone(),
            message_ids: undo.message_ids.clone(),
        });
        self.undoing.push((undo.account, task, undo));
    }

    /// Keeps for `u` again what `account`'s Task `task` was undoing, finished without moving them back: they may be
    /// where they went later, as a server can take a while to show them there.
    pub(super) fn not_undone(&mut self, kit: &mut Kit, account: usize, task: client::Task) {
        let Some(at) = self
            .undoing
            .iter()
            .position(|(of, undoing, _)| (*of, *undoing) == (account, task))
        else {
            return;
        };
        let (_, _, undo) = self.undoing.remove(at);
        self.undo.push(undo);
        kit.error("Not moved back · u tries again");
    }

    /// Takes `targets` out of their Index, as they've gone elsewhere; in the Pager, straight on to the next one, as
    /// `<C-n>` goes, or back to the Index after the last.
    fn taken_out(&mut self, kit: &mut Kit, targets: &Targets) -> Effects {
        let current = kit.current_buffer();
        let in_pager = self.pagers.get(&current).and_then(|pager| {
            let from = pager.from?;
            let opened = pager.opened.clone()?;
            let index = self.indexes.get(&from)?;
            let next = index
                .next(opened.uid, true)
                .or_else(|| index.next(opened.uid, false));
            Some((from, opened, next))
        });
        if let Some((buffer, index)) = self.index_for(targets) {
            index.remove(&targets.uids);
            self.dirty.insert(buffer);
        }
        match in_pager {
            Some((from, opened, Some(next))) => {
                let opened = Opened {
                    uid: next,
                    ..opened
                };
                self.open_message(kit, opened, Some(from), None);
                Vec::new()
            }
            Some((_, _, None)) => self.close_pager(kit, current),
            None => Vec::new(),
        }
    }

    /// The Mailboxes of `account` Messages of `mailbox` can be moved to, in the sidebar's order.
    pub(crate) fn moving_to(&self, account: usize, mailbox: &str) -> Vec<muttrs_mail::Mailbox> {
        sidebar::rows(&self.accounts)
            .into_iter()
            .filter_map(|row| match row {
                Row::Mailbox(at, name) if at == account && name != mailbox => {
                    self.accounts[at].mailbox(&name).cloned()
                }
                _ => None,
            })
            .filter(|other| other.selectable)
            .collect()
    }

    /// Moves `targets` to the Mailbox `to`, by its name on the server.
    pub(crate) fn move_to(&mut self, kit: &mut Kit, targets: Targets, to: &str) -> Effects {
        self.remember_undo(&targets, client::Went::Mailbox(to.to_owned()));
        // The next Message to read asked first, so that it doesn't wait for them to go.
        let effects = self.taken_out(kit, &targets);
        if let Some(state) = self.accounts.get_mut(targets.account) {
            state.ask(client::Request::Move {
                mailbox: targets.mailbox,
                uids: targets.uids,
                to: to.to_owned(),
            });
        }
        effects
    }

    /// `:move NAME`: moves the Messages an action is for to the Mailbox called `name`.
    pub(crate) fn move_named(&mut self, kit: &mut Kit, name: &str) -> Result<Effects, String> {
        let targets = self.targets_or_say(kit)?;
        let to = self
            .mailbox_called(targets.account, name)
            .filter(|mailbox| mailbox.selectable && mailbox.name != targets.mailbox)
            .map(|mailbox| mailbox.name.clone())
            .ok_or_else(|| format!("No other Mailbox called {name}"))?;
        Ok(self.move_to(kit, targets, &to))
    }

    /// How many of `targets` have `account`'s `label`th Label, of how many the Index lists.
    pub(crate) fn labelled(&self, targets: &Targets, label: usize) -> (usize, usize) {
        let (Some(index), Some(label)) = (
            self.mailboxes
                .get(&(targets.account, targets.mailbox.clone()))
                .and_then(|buffer| self.indexes.get(buffer)),
            self.accounts
                .get(targets.account)
                .and_then(|state| state.labels.get(label)),
        ) else {
            return (0, 0);
        };
        let listed: Vec<&muttrs_mail::Envelope> = targets
            .uids
            .iter()
            .filter_map(|&uid| index.find(uid))
            .collect();
        let have = listed
            .iter()
            .filter(|envelope| envelope.labels.contains(&label.name))
            .count();
        (have, listed.len())
    }

    /// Adds the `label`th of the Account's Labels to `targets`: to all of them, unless all have it already, when it's
    /// taken off them.
    pub(crate) fn toggle_label(&mut self, targets: &Targets, label: usize) {
        let Some(chosen) = self
            .accounts
            .get(targets.account)
            .and_then(|state| state.labels.get(label))
            .cloned()
        else {
            return;
        };
        let (have, all) = self.labelled(targets, label);
        let on = have < all || all == 0;
        // As it will be, until the server says.
        if let Some((buffer, index)) = self.index_for(targets) {
            for &uid in &targets.uids {
                if let Some(envelope) = index.find_mut(uid) {
                    tasks::set_label(&mut envelope.labels, &chosen.name, on);
                }
            }
            self.dirty.insert(buffer);
        }
        if let Some(state) = self.accounts.get_mut(targets.account) {
            state.ask(client::Request::Label {
                mailbox: targets.mailbox.clone(),
                uids: targets.uids.clone(),
                label: chosen.mailbox,
                on,
            });
        }
    }

    /// The Mailbox of `account` called `wanted`: by its name for people, its path, or its name on the server.
    pub(crate) fn mailbox_called(
        &self,
        account: usize,
        wanted: &str,
    ) -> Option<&muttrs_mail::Mailbox> {
        self.accounts
            .get(account)?
            .mailboxes
            .iter()
            .find(|mailbox| {
                mailbox.label().eq_ignore_ascii_case(wanted)
                    || mailbox.name.eq_ignore_ascii_case(wanted)
                    || mailbox.path.join("/").eq_ignore_ascii_case(wanted)
            })
    }

    /// Opens the Mailbox called `wanted` (by its name for people, or on the server), of the Account shown.
    pub(crate) fn change_mailbox(&mut self, kit: &mut Kit, wanted: &str) -> Result<(), String> {
        let account = self.account_shown(kit);
        let name = self
            .mailbox_called(account, wanted)
            .map(|mailbox| mailbox.name.clone())
            .ok_or_else(|| format!("No Mailbox called {wanted}"))?;
        self.open_mailbox(kit, account, &name);
        Ok(())
    }

    /// Creates a Mailbox, called `path` (`Work/Reports`), of the Account shown.
    pub(crate) fn create_mailbox(&mut self, kit: &mut Kit, path: &str) -> Result<(), String> {
        let account = self.account_shown(kit);
        if self.mailbox_called(account, path).is_some() {
            return Err(format!("There's a Mailbox called {path} already"));
        }
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::CreateMailbox(path.to_owned()));
        }
        Ok(())
    }

    /// Renames the open Mailbox `to` (`Work/Reports`), unless it has a Role.
    pub(crate) fn rename_mailbox(&mut self, kit: &mut Kit, to: &str) -> Result<(), String> {
        let (_, index) = self
            .current_index(kit)
            .ok_or_else(|| "No Mailbox is open".to_owned())?;
        let (account, mailbox) = (index.account, index.mailbox.clone());
        let role = self
            .accounts
            .get(account)
            .and_then(|state| state.mailbox(&mailbox))
            .and_then(|mailbox| mailbox.role);
        if let Some(role) = role {
            return Err(format!(
                "{} keeps its name, as the Account's {}",
                role_name(role),
                role_name(role)
            ));
        }
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::RenameMailbox {
                mailbox,
                to: to.to_owned(),
            });
            self.renamed = Some((account, to.to_owned()));
        }
        Ok(())
    }

    /// Deletes the Mailbox called `wanted`, and its Messages, unless it has a Role; the Inbox opened in its place
    /// if it was open.
    pub(crate) fn delete_mailbox(&mut self, kit: &mut Kit, wanted: &str) -> Result<(), String> {
        let account = self.account_shown(kit);
        let mailbox = self
            .mailbox_called(account, wanted)
            .cloned()
            .ok_or_else(|| format!("No Mailbox called {wanted}"))?;
        if let Some(role) = mailbox.role {
            return Err(format!(
                "{} is the Account's {}: it isn't to be deleted",
                mailbox.label(),
                role_name(role)
            ));
        }
        let open = self
            .current_index(kit)
            .is_some_and(|(_, index)| index.account == account && index.mailbox == mailbox.name);
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::DeleteMailbox(mailbox.name));
        }
        if open
            && let Some(inbox) = self
                .accounts
                .get(account)
                .and_then(|state| state.with_role(Role::Inbox))
                .map(|inbox| inbox.name.clone())
        {
            self.open_mailbox(kit, account, &inbox);
        }
        Ok(())
    }

    /// The Index the current buffer is, to change, or why there's none.
    fn index_here(&mut self, kit: &Kit) -> Result<(BufferId, &mut Index), String> {
        let current = kit.current_buffer();
        let buffer = if self.indexes.contains_key(&current) {
            current
        } else {
            self.pagers
                .get(&current)
                .and_then(|pager| pager.from)
                .ok_or_else(|| "No Mailbox is open here".to_owned())?
        };
        let index = self
            .indexes
            .get_mut(&buffer)
            .ok_or_else(|| "No Mailbox is open here".to_owned())?;
        Ok((buffer, index))
    }

    /// Limits the Index to the Messages `filter` picks; to none, all show again, if it's empty.
    pub(crate) fn limit_to(&mut self, kit: &mut Kit, filter: &str) -> Result<(), String> {
        let parsed = if filter.is_empty() {
            None
        } else {
            Some((
                filter.to_owned(),
                filter
                    .parse::<Filter>()
                    .map_err(|error| error.to_string())?,
            ))
        };
        let (buffer, index) = self.index_here(kit)?;
        let shown = index.limit(parsed);
        self.dirty.insert(buffer);
        kit.info(match (filter, shown) {
            ("", _) => "All show again".to_owned(),
            (_, 0) => format!("None listed matches {filter}"),
            (_, 1) => format!("1 Message matches {filter}"),
            (_, shown) => format!("{shown} Messages match {filter}"),
        });
        Ok(())
    }

    /// Selects, or unselects, the Messages the Index lists that `filter` picks: all of them, without one.
    pub(crate) fn select_by(
        &mut self,
        kit: &mut Kit,
        on: bool,
        filter: &str,
    ) -> Result<(), String> {
        let parsed = filter
            .parse::<Filter>()
            .map_err(|error| error.to_string())?;
        let (buffer, index) = self.index_here(kit)?;
        let (picked, changed) = index.select_where(on, |envelope| parsed.matches(envelope));
        self.dirty.insert(buffer);
        let done = if on { "selected" } else { "unselected" };
        kit.info(match (picked, changed) {
            (0, _) if filter.is_empty() => "No Message listed".to_owned(),
            (0, _) => format!("No Message listed matches {filter}"),
            (_, 0) if on => "All of those are selected already".to_owned(),
            (_, 0) => "None of those is selected".to_owned(),
            (_, 1) => format!("1 Message {done}"),
            (_, changed) => format!("{changed} Messages {done}"),
        });
        Ok(())
    }

    /// `x`: selects the Message at the cursor, with its thread when that's folded into it, or unselects it.
    pub(crate) fn toggle_selected(&mut self, kit: &mut Kit) -> Result<(), String> {
        let current = kit.current_buffer();
        let index = self
            .indexes
            .get(&current)
            .ok_or_else(|| "Selecting is for the Index".to_owned())?;
        let uids = if index.selected() == 0 {
            self.targets(kit).map(|targets| targets.uids)
        } else {
            // The Selection is what actions are for: the cursor's Message is found anew.
            let Some(Target::Message(opened)) = kit.target_under_cursor() else {
                return Ok(());
            };
            Some(Self::at_cursor(kit, index, opened.uid))
        };
        if let (Some(uids), Some(index)) = (uids, self.indexes.get_mut(&current)) {
            index.toggle_selected(&uids);
            self.dirty.insert(current);
        }
        Ok(())
    }

    /// Orders the Index by `sort`, the other way if `reverse`, or in threads.
    pub(crate) fn sort(
        &mut self,
        kit: &mut Kit,
        sort: Option<Sort>,
        reverse: bool,
    ) -> Result<(), String> {
        let (buffer, index) = self.index_here(kit)?;
        index.sort(sort, reverse);
        let said = index.sorted().unwrap_or(if sort.is_some() {
            "Newest first"
        } else {
            "In threads"
        });
        let said = format!("{}{}", said[..1].to_uppercase(), &said[1..]);
        self.dirty.insert(buffer);
        kit.info(said);
        Ok(())
    }

    /// Shows the Index in threads, or newest first.
    pub(crate) fn toggle_threads(&mut self, kit: &mut Kit) -> Result<(), String> {
        let (buffer, index) = self.index_here(kit)?;
        let threaded = index.toggle_threads();
        self.dirty.insert(buffer);
        kit.info(if threaded {
            "In threads"
        } else {
            "Newest first"
        });
        Ok(())
    }

    /// Asks every Account what's new, connecting again any that's offline.
    pub(crate) fn refresh(&mut self) {
        for account in &mut self.accounts {
            account.ask(client::Request::Refresh);
        }
    }

    /// Forgets what's kept of the open Mailbox, and lists it anew.
    pub(crate) fn resync(&mut self, kit: &Kit) -> Result<(), String> {
        let (_, index) = self
            .current_index(kit)
            .ok_or_else(|| "No Mailbox is open".to_owned())?;
        let (account, mailbox) = (index.account, index.mailbox.clone());
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::Resync(mailbox));
        }
        Ok(())
    }

    /// Searches every Mailbox of the Account shown on the server for `query`, to list where it found Messages.
    pub(crate) fn search_all(&mut self, kit: &Kit, query: &str) {
        let account = self.account_shown(kit);
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::SearchAll {
                query: query.to_owned(),
            });
        }
    }

    /// Searches the open Mailbox on the server for `query`.
    pub(crate) fn search_server(&mut self, kit: &Kit, query: &str) -> Result<(), String> {
        let (_, index) = self
            .current_index(kit)
            .ok_or_else(|| "No Mailbox is open".to_owned())?;
        let (account, mailbox) = (index.account, index.mailbox.clone());
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::Search {
                mailbox,
                query: query.to_owned(),
            });
        }
        Ok(())
    }

    /// Lists what a search of `account`'s `mailbox` found, as an Index of its own, `q` going back.
    pub(super) fn found(
        &mut self,
        kit: &mut Kit,
        account: usize,
        mailbox: &str,
        query: &str,
        envelopes: Vec<muttrs_mail::Envelope>,
        count: usize,
    ) {
        let key = (account, mailbox.to_owned());
        let Some(&listed) = self.mailboxes.get(&key) else {
            return;
        };
        let Some(found) = self
            .indexes
            .get(&listed)
            .map(|index| index.found(query.to_owned(), envelopes, count))
        else {
            return;
        };
        let name = format!("{}?{query}", kit.buffer(listed).name());
        let buffer = if let Some(&buffer) = self.searched.get(&key) {
            kit.buffer_mut(buffer).set_name(name);
            buffer
        } else {
            let buffer = kit.add_buffer(name, Document::new(), false);
            kit.buffer_mut(buffer).set_cursorline(CursorLine::Row);
            self.searched.insert(key, buffer);
            buffer
        };
        self.indexes.insert(buffer, found);
        kit.info(match count {
            0 => "Nothing found".to_owned(),
            1 => "Found 1 Message".to_owned(),
            count => format!("Found {count} Messages"),
        });
        self.close_mini(kit);
        self.show_in_main(kit, buffer);
        self.dirty.insert(buffer);
    }

    /// Lists the Mailboxes of `account` a search of them all for `query` `found` Messages in, how many in each, to
    /// choose one to list what's found there.
    pub(super) fn found_in(
        &mut self,
        kit: &mut Kit,
        account: usize,
        query: &str,
        found: Vec<(String, usize)>,
    ) -> Effects {
        if found.is_empty() {
            kit.info(format!("Nothing found in any Mailbox for {query}"));
            return Vec::new();
        }
        let picker = self.found_picker(kit, account, query, found);
        kit.open_picker(self, picker)
    }

    /// Unsubscribes from the mailing list the Message the Pager shows came through, as it says to: by a POST alone,
    /// by mail (sent at once), or at its page, in the browser.
    pub(crate) fn unsubscribe(&mut self, kit: &mut Kit) -> Result<Effects, String> {
        let pager = self
            .pagers
            .get(&kit.current_buffer())
            .ok_or_else(|| ":unsubscribe is for the Message the Pager shows".to_owned())?;
        let message = Arc::clone(pager.message());
        let account = pager
            .opened
            .as_ref()
            .map(|opened| opened.account)
            .ok_or_else(|| {
                "Unsubscribing needs an Account: run muttrs without `view`".to_owned()
            })?;
        let list = message
            .list
            .clone()
            .unwrap_or_else(|| "the mailing list".to_owned());
        match message.unsubscribe() {
            None => Err("It says no way to unsubscribe (List-Unsubscribe)".to_owned()),
            Some(muttrs_mail::message::Unsubscribe::OneClick(url)) => {
                kit.info(format!("Unsubscribing from {list}…"));
                Ok(vec![Effect::App(Io::Unsubscribe { url, list })])
            }
            Some(muttrs_mail::message::Unsubscribe::Mail(mailto)) => {
                let Some(state) = self.accounts.get_mut(account) else {
                    return Ok(Vec::new());
                };
                let mut draft = muttrs_mail::compose::Draft::new(&state.sender);
                draft.from = state.sender.answering(&message).cloned();
                draft.to = vec![muttrs_mail::Address {
                    name: None,
                    email: mailto.to,
                }];
                draft.subject = mailto.subject.unwrap_or_else(|| "unsubscribe".to_owned());
                draft.body = mailto.body.unwrap_or_default();
                let from = draft
                    .from
                    .as_ref()
                    .or_else(|| state.sender.primary())
                    .map_or("", |from| from.email.as_str());
                let id = muttrs_mail::compose::message_id(from);
                let built = draft
                    .build(&id)
                    .map_err(|error| format!("Not unsubscribed: {error}"))?;
                let task = state.ask_as(
                    client::Request::Send {
                        built: Box::new(built),
                        answers: None,
                    },
                    Some(format!("Unsubscribing from {list}")),
                );
                self.unsubscribing.push((account, task, list));
                Ok(Vec::new())
            }
            Some(muttrs_mail::message::Unsubscribe::Page(url)) => {
                kit.info(format!(
                    "{list}'s page to unsubscribe at is open in the browser"
                ));
                Ok(vec![Effect::OpenUrl(url)])
            }
        }
    }

    /// Pipes the Messages an action is for to `command`.
    pub(crate) fn pipe(&mut self, kit: &mut Kit, command: &str) -> Result<Effects, String> {
        if command.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_then(kit, Then::Pipe(command.to_owned()))
    }

    /// Sends the Messages an action is for on, as they are, to the addresses `to` writes (mutt's bounce).
    pub(crate) fn bounce(&mut self, kit: &mut Kit, to: &str) -> Result<Effects, String> {
        let addresses =
            muttrs_mail::compose::parse_addresses(to).map_err(|error| error.to_string())?;
        if addresses.is_empty() {
            return Err(":bounce takes whom to: :bounce ann@example.com".to_owned());
        }
        self.fetch_then(kit, Then::Bounce(addresses))
    }

    /// Does `then` with the Messages an action is for: the one the Pager shows, as it came; else those of the
    /// Index, the oldest first, once they're fetched.
    fn fetch_then(&mut self, kit: &mut Kit, then: Then) -> Result<Effects, String> {
        if let Some(pager) = self.pagers.get(&kit.current_buffer()) {
            let message = Arc::new(pager.message().raw().to_vec());
            let account = pager.opened.as_ref().map_or(0, |opened| opened.account);
            return Ok(self.then(then, account, &[message]));
        }
        let Targets {
            account,
            mailbox,
            mut uids,
        } = self.targets_or_say(kit)?;
        if let Some(index) = self
            .mailboxes
            .get(&(account, mailbox.clone()))
            .and_then(|buffer| self.indexes.get(buffer))
        {
            uids.sort_by_key(|&uid| (index.find(uid).and_then(|envelope| envelope.date), uid));
        }
        let Some(state) = self.accounts.get_mut(account) else {
            return Ok(Vec::new());
        };
        for &uid in &uids {
            state.ask(client::Request::Body {
                mailbox: mailbox.clone(),
                uid,
            });
        }
        self.piping = Some(Piping {
            then,
            account,
            mailbox,
            uids,
            got: HashMap::new(),
        });
        Ok(Vec::new())
    }

    /// Takes Message `uid` of `account`'s `mailbox`, fetched, if something's to be done with it; does it once
    /// they've all come.
    pub(super) fn pipe_when_fetched(
        &mut self,
        kit: &mut Kit,
        account: usize,
        mailbox: &str,
        uid: u32,
        body: &Result<Arc<Vec<u8>>, String>,
    ) -> Effects {
        let Some(piping) = self.piping.as_mut().filter(|piping| {
            piping.account == account && piping.mailbox == mailbox && piping.uids.contains(&uid)
        }) else {
            return Vec::new();
        };
        match body {
            Ok(bytes) => {
                piping.got.insert(uid, Arc::clone(bytes));
            }
            Err(why) => {
                self.piping = None;
                kit.error(format!("Not done: a Message couldn't be fetched: {why}"));
                return Vec::new();
            }
        }
        if piping.got.len() < piping.uids.len() {
            return Vec::new();
        }
        let Some(piping) = self.piping.take() else {
            return Vec::new();
        };
        let messages: Vec<Arc<Vec<u8>>> = piping
            .uids
            .iter()
            .filter_map(|uid| piping.got.get(uid).cloned())
            .collect();
        self.then(piping.then, piping.account, &messages)
    }

    /// Does `then` with `messages`, of `account`, as RFC 5322 writes them.
    fn then(&mut self, then: Then, account: usize, messages: &[Arc<Vec<u8>>]) -> Effects {
        match then {
            Then::Pipe(command) => {
                let raw: Vec<&[u8]> = messages.iter().map(|message| message.as_slice()).collect();
                // Byte for byte, as they came: an 8bit part in Latin-1 reaches the command as it is.
                let mbox = muttrs_mail::mbox::for_commands(&raw);
                vec![Effect::Shell(
                    wcgw_kit::Shell::new(command).stdin(mbox).wait(),
                )]
            }
            Then::Bounce(to) => {
                let Some(state) = self.accounts.get_mut(account) else {
                    return Vec::new();
                };
                let Some(from) = state.sender.primary().cloned() else {
                    return Vec::new();
                };
                let whom = to
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                for message in messages {
                    let id = muttrs_mail::compose::message_id(&from.email);
                    let built = muttrs_mail::compose::bounced(message, &from, &to, &id);
                    let task = state.ask_as(
                        client::Request::Bounce {
                            built: Box::new(built),
                        },
                        Some(format!("Bouncing a Message to {whom}")),
                    );
                    self.bouncing.push((account, task, whom.clone()));
                }
                Vec::new()
            }
        }
    }

    /// Runs `command` (`:shell`, `!`) with the terminal, which comes back once Enter's pressed after it; the shell
    /// itself (`$SHELL`) if it's empty.
    pub(crate) fn shell(command: &str) -> Effects {
        if command.is_empty() {
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|shell| !shell.trim().is_empty())
                .unwrap_or_else(|| "sh".to_owned());
            return vec![Effect::Shell(wcgw_kit::Shell::new(shell))];
        }
        vec![Effect::Shell(wcgw_kit::Shell::new(command).wait())]
    }

    /// Opens or saves Attachment `part` of the Pager in `pager`.
    pub(super) fn attachment(
        &self,
        pager: BufferId,
        part: usize,
        save: bool,
    ) -> Result<Effects, String> {
        let pager = self
            .pagers
            .get(&pager)
            .ok_or_else(|| "That Message isn't shown any more".to_owned())?;
        let attachments = pager.attachments();
        let part = attachments
            .get(part)
            .ok_or_else(|| "No such Attachment".to_owned())?;
        let bytes = Arc::new(pager.message().bytes(part));
        let name = part.file_name();
        Ok(vec![Effect::App(if save {
            Io::Save {
                directory: self.config.download_dir(),
                name,
                bytes,
            }
        } else {
            Io::OpenFile { name, bytes }
        })])
    }

    /// `K`: in the Pager, every header as written, or those people read; in the Index, a card of the Message's
    /// Envelope.
    pub(crate) fn details(&mut self, kit: &mut Kit) -> Result<(), String> {
        let current = kit.current_buffer();
        if let Some(pager) = self.pagers.get_mut(&current) {
            pager.toggle_headers();
            self.dirty.insert(current);
            return Ok(());
        }
        let index = self
            .indexes
            .get(&current)
            .ok_or_else(|| "No Message here".to_owned())?;
        let Some(Target::Message(opened)) = kit.target_under_cursor() else {
            return Err("No Message here".to_owned());
        };
        let envelope = index
            .find(opened.uid)
            .ok_or_else(|| "No Message here".to_owned())?;
        let list = |addresses: &[muttrs_mail::Address]| {
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines = Vec::new();
        let mut field = |name: &str, value: String| {
            if !value.is_empty() {
                lines.push(Line::Text(vec![
                    Span::new(format!("{name:<9}"), hl::DIM),
                    Span::new(value, wcgw_kit::hl::Group::NORMAL_FLOAT),
                ]));
            }
        };
        field("From", list(&envelope.from));
        field("To", list(&envelope.to));
        field("Cc", list(&envelope.cc));
        field(
            "Date",
            envelope.date.map_or_else(String::new, |date| {
                date.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%a %-d %b %Y, %H:%M")
                    .to_string()
            }),
        );
        field("Subject", envelope.subject.clone());
        field("Labels", envelope.labels.join(", "));
        field(
            "Size",
            crate::pager::size(usize::try_from(envelope.size).unwrap_or(usize::MAX)),
        );
        let mut doc = Document::new();
        doc.push_back("details", lines);
        kit.show_hover(doc);
        Ok(())
    }

    /// `:raw`: the Message the Pager shows as it came, or as read.
    pub(crate) fn toggle_raw(&mut self, kit: &Kit) -> Result<(), String> {
        let current = kit.current_buffer();
        let pager = self
            .pagers
            .get_mut(&current)
            .ok_or_else(|| ":raw is for the Message the Pager shows".to_owned())?;
        pager.toggle_source();
        self.dirty.insert(current);
        Ok(())
    }

    /// `:body`: the other body of the Message the Pager shows, HTML or plain text.
    pub(crate) fn toggle_body(&mut self, kit: &Kit) -> Result<(), String> {
        let current = kit.current_buffer();
        let pager = self
            .pagers
            .get_mut(&current)
            .ok_or_else(|| ":body is for the Message the Pager shows".to_owned())?;
        if !pager.toggle_body() {
            return Err("This Message has one body only".to_owned());
        }
        self.dirty.insert(current);
        Ok(())
    }

    /// `I`: loads the Remote Content of the Message the Pager shows.
    pub(crate) fn load_remote(&mut self, kit: &Kit) -> Result<Effects, String> {
        let current = kit.current_buffer();
        let pager = self
            .pagers
            .get_mut(&current)
            .ok_or_else(|| ":remote is for the Message the Pager shows".to_owned())?;
        match pager.allow_remote() {
            crate::pager::Request::None => Err("No Remote Content to load".to_owned()),
            crate::pager::Request::Fetch(images) => {
                self.dirty.insert(current);
                Ok(images
                    .into_iter()
                    .map(|(source, url)| Effect::App(Io::Remote { source, url }))
                    .collect())
            }
        }
    }

    /// `gx`: opens the Link under the cursor in the browser, or `gy` copies it (`copy`); without one, lists the
    /// Links, `<CR>` opening one and `<C-y>` copying it.
    pub(crate) fn browse(&mut self, kit: &mut Kit, copy: bool) -> Result<Effects, String> {
        if let Some(Target::Link(url)) = kit.target_under_cursor() {
            return Ok(if copy {
                super::pickers::copy(kit, url)
            } else {
                vec![Effect::OpenUrl(url)]
            });
        }
        let picker = self.links_picker(kit)?;
        Ok(kit.open_picker(self, picker))
    }

    /// The Message the Pager shows, or the Index's cursor is on, by where it is: what writing about it is about.
    pub(crate) fn message_here(&self, kit: &Kit) -> Option<(Opened, Option<BufferId>)> {
        let current = kit.current_buffer();
        if let Some(pager) = self.pagers.get(&current) {
            return Some((pager.opened.clone()?, pager.from));
        }
        self.indexes.get(&current)?;
        match kit.target_under_cursor() {
            Some(Target::Message(opened)) => Some((opened, Some(current))),
            _ => None,
        }
    }

    /// Takes what came of sending the Draft (or a bounce, or an unsubscribing) of `account`'s Task `task`.
    pub(super) fn sent(
        &mut self,
        kit: &mut Kit,
        account: usize,
        task: client::Task,
        sent: Result<(), String>,
    ) {
        if let Some(at) = self
            .bouncing
            .iter()
            .position(|(of, asked, _)| *of == account && *asked == task)
        {
            let (_, _, whom) = self.bouncing.remove(at);
            match sent {
                Ok(()) => kit.info(format!("Bounced to {whom}")),
                Err(why) => kit.error(format!("Not bounced: {why}")),
            }
            return;
        }
        if let Some(at) = self
            .unsubscribing
            .iter()
            .position(|(of, asked, _)| *of == account && *asked == task)
        {
            let (_, _, list) = self.unsubscribing.remove(at);
            match sent {
                Ok(()) => kit.info(format!("Asked {list} by mail to unsubscribe you")),
                Err(why) => kit.error(format!("Not unsubscribed from {list}: {why}")),
            }
            return;
        }
        self.draft_sent(kit, account, task, sent);
    }

    /// Takes in what voight says: that its model is loaded, or why it couldn't be, or an Assessment.
    pub(super) fn voight_said(&mut self, kit: &mut Kit, said: crate::assessment::Event) -> Effects {
        let Some(assessments) = &mut self.assessments else {
            return Vec::new();
        };
        match said {
            crate::assessment::Event::Loaded { fallback } => {
                assessments.loaded(true);
                if let Some(fallback) = fallback {
                    kit.warn(fallback);
                }
            }
            crate::assessment::Event::Failed(why) => {
                assessments.loaded(false);
                kit.error(format!("voight couldn't load its model: {why}"));
            }
            crate::assessment::Event::Assessed { key, assessment } => {
                assessments.assessed(key, assessment);
                if self.heatmap {
                    self.dirty.extend(self.pagers.keys().copied());
                }
            }
        }
        Vec::new()
    }
}
