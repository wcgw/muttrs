//! Writing: new Messages, replies and forwards, edited in `$EDITOR` as the kit hands it the terminal, then shown as
//! they'll be sent, in a buffer of their own, until they're sent, postponed or discarded.

use std::sync::Arc;

use muttrs_mail::client::{self, Task};
use muttrs_mail::{Message, Role};

use super::{Document, Effect, Effects, Kit, Muttrs};
use crate::compose::{self, Compose};

/// What's being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Writing {
    Reply,
    ReplyAll,
    /// A reply to the mailing list the Message came through.
    ReplyList,
    Forward,
    /// A forward with the Message as an Attachment of its own.
    ForwardAttached,
    /// A Postponed Draft, resumed.
    Resume,
}

/// What the status line says while `draft` is sent: its subject, and whom it's to.
fn sending(draft: &muttrs_mail::compose::Draft) -> String {
    let to = draft.to.first().map(|address| {
        address
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| address.email.clone())
    });
    match (draft.subject.trim(), to) {
        ("", Some(to)) => format!("Sending to {to}"),
        ("", None) => "Sending".to_owned(),
        (subject, Some(to)) => format!("Sending {subject} to {to}"),
        (subject, None) => format!("Sending {subject}"),
    }
}

impl Muttrs {
    /// `:compose`: a new Message to `to`, as written, asking whom to first if it says nobody, then its subject, as
    /// mutt does, before the editor.
    pub(crate) fn compose(&mut self, kit: &mut Kit, to: &str) -> Result<Effects, String> {
        if self.standalone {
            return Err("Writing needs an Account: run muttrs without `view`".to_owned());
        }
        if to.trim().is_empty() {
            kit.prompt("To: ", false, |_: &mut Self, kit, to| {
                if to.trim().is_empty() {
                    return Vec::new();
                }
                Self::addressed(kit, &to)
            });
            return Ok(Vec::new());
        }
        Ok(Self::addressed(kit, to))
    }

    /// Takes whom a new Message is to, and asks its subject.
    fn addressed(kit: &mut Kit, to: &str) -> Effects {
        match muttrs_mail::compose::parse_addresses(to.trim().trim_end_matches(',')) {
            Ok(to) => {
                kit.prompt(
                    "Subject: ",
                    false,
                    move |muttrs: &mut Self, kit, subject| muttrs.subject(kit, to, &subject),
                );
            }
            Err(error) => kit.error(error.to_string()),
        }
        Vec::new()
    }

    /// Takes a new Message's subject, and opens it in the editor.
    fn subject(&mut self, kit: &mut Kit, to: Vec<muttrs_mail::Address>, subject: &str) -> Effects {
        let account = self.account_shown(kit);
        let Some(state) = self.accounts.get(account) else {
            return Vec::new();
        };
        let mut draft = muttrs_mail::compose::Draft::new(&state.sender);
        draft.to = to;
        subject.trim().clone_into(&mut draft.subject);
        self.start_draft(kit, Compose::new(account, draft, None))
    }

    /// Starts writing about the Message the Pager shows or the Index's cursor is on: once it's fetched, from the
    /// Index.
    pub(crate) fn write(&mut self, kit: &mut Kit, writing: Writing) -> Result<Effects, String> {
        if self.standalone {
            return Err("Writing needs an Account: run muttrs without `view`".to_owned());
        }
        if self.compose.is_some() {
            return Err(
                "A Draft is being written already: :send, :postpone or :discard it".to_owned(),
            );
        }
        let (opened, from) = self
            .message_here(kit)
            .ok_or_else(|| "No Message here to write about".to_owned())?;
        if let Some(pager) = self.pagers.get(&kit.current_buffer()) {
            let message = Arc::clone(pager.message());
            return Ok(self.write_about(kit, writing, opened.account, &message, opened.uid));
        }
        let uid = opened.uid;
        self.open_message(kit, opened, from, Some(uid), Some(writing));
        Ok(Vec::new())
    }

    /// Writes about `message`, `account`'s Message `uid` of the Mailbox shown, as `writing` says.
    pub(super) fn write_about(
        &mut self,
        kit: &mut Kit,
        writing: Writing,
        account: usize,
        message: &Message,
        uid: u32,
    ) -> Effects {
        let Some(state) = self.accounts.get(account) else {
            return Vec::new();
        };
        let me = state.sender.clone();
        let text = compose::quotable(message);
        let mailbox = self
            .current_index(kit)
            .map_or_default(|(_, index)| index.mailbox.clone());
        let (draft, answers) = match writing {
            Writing::Reply | Writing::ReplyAll => (
                muttrs_mail::compose::Draft::reply(
                    message,
                    &me,
                    writing == Writing::ReplyAll,
                    &text,
                ),
                Some((mailbox, uid)),
            ),
            Writing::ReplyList => {
                let Some(list) = &message.list_post else {
                    kit.info("It didn't come through a mailing list that takes posts (List-Post)");
                    return Vec::new();
                };
                (
                    muttrs_mail::compose::Draft::reply_to_list(message, &me, list, &text),
                    Some((mailbox, uid)),
                )
            }
            Writing::ForwardAttached => (
                muttrs_mail::compose::Draft::forward_attached(message, &me),
                None,
            ),
            Writing::Forward => (
                muttrs_mail::compose::Draft::forward(message, &me, &text),
                None,
            ),
            Writing::Resume => {
                // A Draft kept is its Markdown as plain text, and the HTML it made.
                let markdown = message.plain().map_or(text, |(markdown, _)| markdown);
                let draft = muttrs_mail::compose::Draft::resumed(message, &markdown);
                let mut compose = Compose::new(account, draft, None);
                compose.replaces = Some((mailbox, uid));
                return self.start_draft(kit, compose);
            }
        };
        self.start_draft(kit, Compose::new(account, draft, answers))
    }

    /// Opens `compose`'s Draft in `$EDITOR`.
    fn start_draft(&mut self, kit: &mut Kit, mut compose: Compose) -> Effects {
        if self.compose.is_some() {
            kit.error("A Draft is being written already");
            return Vec::new();
        }
        let text = compose.text();
        self.compose = Some(compose);
        vec![Effect::Edit { text }]
    }

    /// `<CR>` on the Draft, `:draft`: edits it again.
    pub(crate) fn edit_draft(&mut self) -> Effects {
        match &mut self.compose {
            Some(compose) => vec![Effect::Edit {
                text: compose.text(),
            }],
            None => Vec::new(),
        }
    }

    /// Takes the Draft back from the editor, which saved it, or not.
    pub(super) fn draft_edited(&mut self, kit: &mut Kit, saved: Result<String, String>) -> Effects {
        let Some(compose) = &mut self.compose else {
            return Vec::new();
        };
        let text = match saved {
            Ok(text) => text,
            Err(why) => {
                kit.error(format!("{why}: the Draft is as it was"));
                if !compose.was_edited() {
                    self.compose = None;
                }
                return Vec::new();
            }
        };
        match compose.saved(&text) {
            Err(error) => {
                kit.error(format!("{error:#}; <CR> on the Draft edits it again"));
            }
            Ok(false) => {
                self.compose = None;
                self.close_draft(kit);
                kit.info("Not sent: the Draft is as it was");
                return Vec::new();
            }
            Ok(true) => {}
        }
        if compose.draft.is_empty() {
            self.compose = None;
            self.close_draft(kit);
            kit.info("Not sent: the Draft says nothing");
            return Vec::new();
        }
        self.show_draft(kit);
        Vec::new()
    }

    /// Shows the Draft as it will be sent.
    fn show_draft(&mut self, kit: &mut Kit) {
        let buffer = if let Some(buffer) = self.draft {
            buffer
        } else {
            let buffer = kit.add_buffer("draft", Document::new(), false);
            self.draft = Some(buffer);
            buffer
        };
        self.dirty.insert(buffer);
        if kit.current_buffer() != buffer {
            self.show_in_main(kit, buffer);
        }
    }

    /// Goes back from the Draft to what was shown before, its buffer emptied.
    fn close_draft(&mut self, kit: &mut Kit) {
        if let Some(buffer) = self.draft {
            *kit.buffer_mut(buffer).doc_mut() = Document::new();
            if kit.current_buffer() == buffer {
                self.back_from(kit);
            }
        }
    }

    /// `:send`: sends the Draft, in the background: it's put away meanwhile, and comes back if it can't be sent.
    pub(crate) fn send(&mut self, kit: &mut Kit) -> Result<Effects, String> {
        let compose = self
            .compose
            .as_ref()
            .ok_or_else(|| "No Draft to send".to_owned())?;
        let state = self
            .accounts
            .get_mut(compose.account)
            .ok_or_else(|| "No Account to send from".to_owned())?;
        // At the domain of the address it's from.
        let from = compose
            .draft
            .from
            .as_ref()
            .or_else(|| state.sender.primary())
            .map_or("", |from| from.email.as_str());
        let id = muttrs_mail::compose::message_id(from);
        let built = compose
            .draft
            .build(&id)
            .map_err(|error| format!("Not sent: {error}"))?;
        let task = state.ask_as(
            client::Request::Send {
                built: Box::new(built),
                answers: compose.answers.clone(),
            },
            Some(sending(&compose.draft)),
        );
        if let Some(compose) = self.compose.take() {
            self.sending.push((task, compose));
        }
        self.close_draft(kit);
        Ok(Vec::new())
    }

    /// Takes what came of sending the Draft of `account`'s Task `task`: done with it, or it's shown again.
    pub(super) fn draft_sent(
        &mut self,
        kit: &mut Kit,
        account: usize,
        task: Task,
        sent: Result<(), String>,
    ) {
        let Some(at) = self
            .sending
            .iter()
            .position(|(sending, compose)| *sending == task && compose.account == account)
        else {
            return;
        };
        let (_, compose) = self.sending.remove(at);
        match sent {
            Ok(()) => {
                let subject = compose.draft.subject.trim();
                kit.info(if subject.is_empty() {
                    "Sent".to_owned()
                } else {
                    format!("Sent {subject}")
                });
                self.expunge_replaced(&compose);
            }
            Err(why) => {
                kit.error(format!("Not sent, the Draft's back: {why}"));
                self.unsent.push(compose);
            }
        }
    }

    /// Shows again a Draft that couldn't be sent, once no other is written.
    pub(super) fn bring_back_unsent(&mut self, kit: &mut Kit) {
        if self.compose.is_none() && !self.unsent.is_empty() {
            self.compose = Some(self.unsent.remove(0));
            self.show_draft(kit);
        }
    }

    /// Expunges the Postponed Draft `compose` was resumed from: it's sent, or kept again.
    fn expunge_replaced(&mut self, compose: &Compose) {
        if let (Some((mailbox, uid)), Some(state)) =
            (&compose.replaces, self.accounts.get_mut(compose.account))
        {
            state.ask(client::Request::Expunge {
                mailbox: mailbox.clone(),
                uids: vec![*uid],
            });
        }
    }

    /// `:resume`: resumes the Postponed Draft the Pager shows, or the Index's cursor is on, in the Drafts Mailbox.
    pub(crate) fn resume(&mut self, kit: &mut Kit) -> Result<Effects, String> {
        let in_drafts = self.current_index(kit).is_some_and(|(_, index)| {
            self.accounts
                .get(index.account)
                .and_then(|state| state.mailbox(&index.mailbox))
                .is_some_and(|mailbox| mailbox.role == Some(Role::Drafts))
        });
        if !in_drafts {
            return Err(":resume resumes a Postponed Draft, in the Drafts Mailbox".to_owned());
        }
        self.write(kit, Writing::Resume)
    }

    /// `:postpone`: saves the Draft to the Drafts Mailbox, to finish later.
    pub(crate) fn postpone(&mut self, kit: &mut Kit) -> Result<Effects, String> {
        let compose = self
            .compose
            .as_ref()
            .ok_or_else(|| "No Draft to postpone".to_owned())?;
        let state = self
            .accounts
            .get_mut(compose.account)
            .ok_or_else(|| "No Account to postpone to".to_owned())?;
        let from = compose
            .draft
            .from
            .as_ref()
            .or_else(|| state.sender.primary())
            .map_or("", |from| from.email.as_str());
        let id = muttrs_mail::compose::message_id(from);
        let built = compose
            .draft
            .build(&id)
            .map_err(|error| format!("Not postponed: {error}"))?;
        state.ask(client::Request::Append {
            role: Role::Drafts,
            message: built.kept,
            draft: true,
        });
        if let Some(compose) = self.compose.take() {
            self.expunge_replaced(&compose);
        }
        self.close_draft(kit);
        kit.info("Postponed to Drafts");
        Ok(Vec::new())
    }

    /// `:attach FILE`: attaches the file at `path` to the Draft; without one, asks which.
    pub(crate) fn attach(&mut self, kit: &mut Kit, path: &str) -> Result<Effects, String> {
        if self.compose.is_none() {
            return Err("No Draft to attach to".to_owned());
        }
        if path.is_empty() {
            kit.prompt("Attach: ", false, |muttrs: &mut Self, kit, path| {
                if let Err(error) = muttrs.attach(kit, path.trim()) {
                    kit.error(error);
                }
                Vec::new()
            });
            return Ok(Vec::new());
        }
        let path = crate::xdg::expand(path);
        if !path.is_file() {
            return Err(format!("{} isn't a file", path.display()));
        }
        if let Some(compose) = &mut self.compose {
            compose
                .draft
                .attachments
                .push(muttrs_mail::compose::Attachment::file(path));
        }
        self.dirty.extend(self.draft);
        Ok(Vec::new())
    }

    /// `:detach`: takes the Draft's last Attachment off.
    pub(crate) fn detach(&mut self) -> Result<(), String> {
        let compose = self
            .compose
            .as_mut()
            .ok_or_else(|| "No Draft to detach from".to_owned())?;
        compose
            .draft
            .attachments
            .pop()
            .ok_or_else(|| "The Draft has no Attachments".to_owned())?;
        self.dirty.extend(self.draft);
        Ok(())
    }

    /// `q` on the Draft: asks before discarding it.
    pub(super) fn ask_to_discard(kit: &mut Kit) -> Effects {
        kit.confirm(
            "Discard the Draft? (y/n)",
            None,
            |muttrs: &mut Self, kit, answer| {
                if answer == 'y' {
                    muttrs.discard(kit);
                }
                Vec::new()
            },
        );
        Vec::new()
    }

    /// `:discard`: drops the Draft.
    pub(crate) fn discard(&mut self, kit: &mut Kit) {
        if self.compose.take().is_some() {
            self.close_draft(kit);
            kit.info("Discarded");
        }
    }
}
