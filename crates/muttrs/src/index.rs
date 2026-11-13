//! The Index: the Envelopes of the open Mailbox, one a row, newest first, or in threads, each under what it
//! replies to and threads with the latest replies first. It's a kit buffer: a block per thread (per Message, out of
//! threads), its rows laid out for the window's width, and a fold over each thread's rows, which `zc` closes into
//! the row heading it.

use std::collections::HashSet;

use jiff::Zoned;
use jiff::tz::TimeZone;
use muttrs_mail::thread::{self, Branch};
use muttrs_mail::{Envelope, Role};
use unicode_width::UnicodeWidthStr as _;
use wcgw_kit::doc::{BlockKey, Content, Fold};

use crate::app::{Kit, Line, Opened, Span, Target};
use crate::filter::Filter;
use crate::hl::{self, Group};
use crate::icons;

/// Where asking for older Envelopes stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum More {
    Idle,
    /// Asked, with this many listed.
    Asked(usize),
    /// Asked, and none came.
    Exhausted,
}

/// A row of the Index.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    /// The Envelope it shows, by its index.
    envelope: usize,
    /// The tree drawn before its subject.
    tree: String,
    /// The UID of the Message heading its thread; its own, flat.
    head: u32,
    /// Whether its subject is what it replies to's, and isn't shown again: an arrow shows instead.
    repeated: bool,
}

/// What the Index's rows are ordered by when they aren't in threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sort {
    /// Newest first.
    Date,
    /// By the name, else the address, of whom they're from (to, in Sent).
    Sender,
    /// By subject, without its `Re:`.
    Subject,
    /// Largest first.
    Size,
}

impl Sort {
    /// The sort a letter names, as mutt's `o` takes it: `t` for threads, which is none.
    pub(crate) fn named(name: &str) -> Result<Option<Self>, String> {
        match name {
            "d" | "date" => Ok(Some(Self::Date)),
            "f" | "from" | "sender" => Ok(Some(Self::Sender)),
            "s" | "subject" => Ok(Some(Self::Subject)),
            "z" | "size" => Ok(Some(Self::Size)),
            "t" | "threads" => Ok(None),
            _ => Err(format!(
                "Not a sort: {name}: date, from, subject, size or threads"
            )),
        }
    }

    /// The names `:sort` completes.
    pub(crate) const NAMES: [&str; 5] = ["date", "from", "subject", "size", "threads"];
}

/// What the Index draws with, from the kit: its icons, and the time now.
pub(crate) struct Look {
    unread: String,
    flagged: String,
    answered: String,
    attachment: String,
    now: Zoned,
}

impl Look {
    pub(crate) fn of(kit: &Kit) -> Self {
        Self {
            unread: kit.icon(icons::UNREAD),
            flagged: kit.icon(icons::FLAGGED),
            answered: kit.icon(icons::ANSWERED),
            attachment: kit.icon(icons::ATTACHMENT),
            now: Zoned::now(),
        }
    }
}

// Each bool says something of its own of the Index: what it lists, how, and whether it has come.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Index {
    /// The Account the Mailbox is of, by its place in the sidebar.
    pub(crate) account: usize,
    /// The Mailbox's name on the server.
    pub(crate) mailbox: String,
    /// The Mailbox's name for people.
    pub(crate) label: String,
    /// Whether the Mailbox holds what the user sent, so that rows show whom to rather than whom from.
    sent: bool,
    /// The Envelopes listed, newest first.
    envelopes: Vec<Envelope>,
    rows: Vec<Row>,
    /// Whether rows are in threads.
    threaded: bool,
    /// Whether a reply's subject is left out where it's what it replies to's, as mutt's `hide_thread_subject`.
    hide_subjects: bool,
    /// The Messages selected, to act on together, by UID.
    selected: HashSet<u32>,
    /// How many Messages the Mailbox has, of which `envelopes` are the newest.
    pub(crate) exists: u32,
    /// Whether the Envelopes have come.
    pub(crate) loaded: bool,
    /// Where asking for older Envelopes stands.
    more: More,
    /// What a search on the server found, if that's what this lists: its query, and how many it found.
    pub(crate) found: Option<(String, usize)>,
    /// The Filter the rows are limited to, as written and read: only the Messages it picks show.
    limit: Option<(String, Filter)>,
    /// What the rows are ordered by when they aren't in threads, and whether the other way.
    sort: Sort,
    reverse: bool,
}

impl Index {
    pub(crate) fn new(
        account: usize,
        mailbox: String,
        label: String,
        role: Option<Role>,
        threaded: bool,
    ) -> Self {
        Self {
            account,
            mailbox,
            label,
            sent: matches!(role, Some(Role::Sent | Role::Drafts)),
            envelopes: Vec::new(),
            rows: Vec::new(),
            threaded,
            hide_subjects: false,
            selected: HashSet::new(),
            exists: 0,
            loaded: false,
            more: More::Idle,
            found: None,
            limit: None,
            sort: Sort::Date,
            reverse: false,
        }
    }

    /// The Index, leaving a reply's subject out where it's what it replies to's, if `hide`.
    pub(crate) fn hiding_repeated_subjects(mut self, hide: bool) -> Self {
        self.hide_subjects = hide;
        self
    }

    /// Takes the Envelopes the server listed.
    pub(crate) fn set(&mut self, mut envelopes: Vec<Envelope>, exists: u32) {
        envelopes.sort_by(|a, b| b.date.cmp(&a.date).then(b.uid.cmp(&a.uid)));
        self.more = match self.more {
            // Asked, and none came: there are none to list, whatever the count says.
            More::Asked(before) if envelopes.len() <= before => More::Exhausted,
            More::Asked(_) => More::Idle,
            other => other,
        };
        self.envelopes = envelopes;
        self.exists = exists;
        self.loaded = true;
        self.arrange();
    }

    /// What a search of this Index's Mailbox for `query` found: `envelopes`, the newest of `count`.
    pub(crate) fn found(&self, query: String, envelopes: Vec<Envelope>, count: usize) -> Self {
        let mut found = Self::new(
            self.account,
            self.mailbox.clone(),
            self.label.clone(),
            None,
            self.threaded,
        );
        found.sent = self.sent;
        found.hide_subjects = self.hide_subjects;
        found.set(envelopes, u32::try_from(count).unwrap_or(u32::MAX));
        found.more = More::Exhausted;
        found.found = Some((query, count));
        found
    }

    /// Lays the rows out again.
    fn arrange(&mut self) {
        let mut shown: Vec<usize> = (0..self.envelopes.len())
            .filter(|&envelope| self.within_limit(&self.envelopes[envelope]))
            .collect();
        if !self.threaded {
            self.order(&mut shown);
        }
        self.rows = if self.threaded {
            self.threads(&shown)
        } else {
            shown
                .into_iter()
                .map(|envelope| Row {
                    envelope,
                    tree: String::new(),
                    head: self.envelopes[envelope].uid,
                    repeated: false,
                })
                .collect()
        };
    }

    /// The rows of the Envelopes `shown`, by their place, in threads among themselves.
    fn threads(&self, shown: &[usize]) -> Vec<Row> {
        let listed: Vec<Envelope>;
        let (envelopes, shown): (&[Envelope], Option<&[usize]>) =
            if shown.len() == self.envelopes.len() {
                (&self.envelopes, None)
            } else {
                listed = shown.iter().map(|&at| self.envelopes[at].clone()).collect();
                (&listed, Some(shown))
            };
        let place = |envelope: usize| shown.map_or(envelope, |shown| shown[envelope]);
        let threaded = thread::threads(envelopes);
        let mut rows: Vec<Row> = Vec::with_capacity(threaded.len());
        let mut head = 0;
        // The subjects of the row's ancestors, by depth: its parent's last.
        let mut ancestors: Vec<&str> = Vec::new();
        for message in &threaded {
            let envelope = &envelopes[message.envelope];
            ancestors.truncate(message.depth);
            let subject = base_subject(&envelope.subject);
            let repeated = self.hide_subjects
                && message.depth > 0
                && ancestors
                    .last()
                    .is_some_and(|parent| parent.eq_ignore_ascii_case(subject));
            ancestors.push(subject);
            if message.depth == 0 {
                head = envelope.uid;
            }
            let tree = message
                .branches
                .iter()
                .map(|branch| match branch {
                    Branch::Through => "│ ",
                    Branch::Blank => "  ",
                    Branch::Tee => "├─",
                    Branch::Corner => "└─",
                })
                .collect();
            rows.push(Row {
                envelope: place(message.envelope),
                tree,
                head,
                repeated,
            });
        }
        rows
    }

    /// Orders the Envelopes `shown`, by their place (newest first), by the sort: ties stay newest first.
    fn order(&self, shown: &mut [usize]) {
        let sender = |envelope: &Envelope| {
            let whom = if self.sent {
                envelope.to.first()
            } else {
                envelope.from.first()
            };
            whom.map_or_else(String::new, |address| {
                address
                    .name
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&address.email)
                    .trim_matches(['"', '\''])
                    .to_lowercase()
            })
        };
        let envelopes = &self.envelopes;
        match self.sort {
            // As they're kept.
            Sort::Date => {}
            Sort::Sender => shown.sort_by_cached_key(|&at| sender(&envelopes[at])),
            Sort::Subject => {
                shown.sort_by_cached_key(|&at| base_subject(&envelopes[at].subject).to_lowercase());
            }
            Sort::Size => shown.sort_by_key(|&at| std::cmp::Reverse(envelopes[at].size)),
        }
        if self.reverse {
            shown.reverse();
        }
    }

    /// Orders the rows by `sort` (the other way if `reverse`), out of threads; or in threads, if it's None.
    pub(crate) fn sort(&mut self, sort: Option<Sort>, reverse: bool) {
        match sort {
            Some(sort) => {
                self.sort = sort;
                self.reverse = reverse;
                self.threaded = false;
            }
            None => self.threaded = true,
        }
        self.arrange();
    }

    /// What the rows are ordered by, when they aren't in threads, nor newest first, for people.
    pub(crate) fn sorted(&self) -> Option<&'static str> {
        if self.threaded {
            return None;
        }
        Some(match (self.sort, self.reverse) {
            (Sort::Date, false) => return None,
            (Sort::Date, true) => "oldest first",
            (Sort::Sender, false) => "by sender",
            (Sort::Sender, true) => "by sender, Z to A",
            (Sort::Subject, false) => "by subject",
            (Sort::Subject, true) => "by subject, Z to A",
            (Sort::Size, false) => "largest first",
            (Sort::Size, true) => "smallest first",
        })
    }

    /// Shows rows in threads, or not.
    pub(crate) fn toggle_threads(&mut self) -> bool {
        self.threaded = !self.threaded;
        self.arrange();
        self.threaded
    }

    /// Selects the Messages `uids`, or unselects them if the first is selected.
    pub(crate) fn toggle_selected(&mut self, uids: &[u32]) {
        let Some(first) = uids.first() else { return };
        if self.selected.contains(first) {
            for uid in uids {
                self.selected.remove(uid);
            }
        } else {
            self.selected.extend(uids);
        }
    }

    /// Selects, or unselects, the Messages listed that `picks` picks, folded ones too: how many it picks, and how
    /// many of those it selected, or unselected, that weren't, or were.
    pub(crate) fn select_where(
        &mut self,
        on: bool,
        picks: impl Fn(&Envelope) -> bool,
    ) -> (usize, usize) {
        let (mut picked, mut changed) = (0, 0);
        let shown: Vec<&Envelope> = self
            .envelopes
            .iter()
            .filter(|envelope| self.within_limit(envelope) && picks(envelope))
            .collect();
        for envelope in shown {
            picked += 1;
            let did = if on {
                self.selected.insert(envelope.uid)
            } else {
                self.selected.remove(&envelope.uid)
            };
            changed += usize::from(did);
        }
        (picked, changed)
    }

    /// Limits the rows to the Messages `limit` picks, as written and read; or not, if it's None. How many show.
    pub(crate) fn limit(&mut self, limit: Option<(String, Filter)>) -> usize {
        self.limit = limit;
        self.arrange();
        self.envelopes
            .iter()
            .filter(|envelope| self.within_limit(envelope))
            .count()
    }

    /// The Filter the rows are limited to, as written, if they are.
    pub(crate) fn limited(&self) -> Option<&str> {
        self.limit.as_ref().map(|(written, _)| written.as_str())
    }

    /// Whether `envelope` shows, within the limit, if there's one.
    fn within_limit(&self, envelope: &Envelope) -> bool {
        self.limit
            .as_ref()
            .is_none_or(|(_, filter)| filter.matches(envelope))
    }

    /// The Envelopes listed, in no particular order.
    pub(crate) fn envelopes(&self) -> &[Envelope] {
        &self.envelopes
    }

    /// How many Messages are selected.
    pub(crate) fn selected(&self) -> usize {
        self.selected.len()
    }

    /// The UIDs of the Messages selected, if any.
    pub(crate) fn selection(&self) -> Vec<u32> {
        let mut uids: Vec<u32> = self.selected.iter().copied().collect();
        uids.sort_unstable();
        uids
    }

    /// The UIDs of the Messages of the thread Message `uid` heads, it first: what's done to it when it's folded.
    pub(crate) fn thread_of(&self, uid: u32) -> Vec<u32> {
        let head = self
            .rows
            .iter()
            .find(|row| self.envelopes[row.envelope].uid == uid)
            .map_or(uid, |row| row.head);
        self.rows
            .iter()
            .filter(|row| row.head == head)
            .map(|row| self.envelopes[row.envelope].uid)
            .collect()
    }

    /// Whether to ask for older Envelopes, the cursor being `near_end` of those listed: the Mailbox has more. Asking
    /// is noted, so that it's asked once.
    pub(crate) fn wants_more(&mut self, near_end: bool) -> bool {
        let has_more = u32::try_from(self.envelopes.len()).unwrap_or(u32::MAX) < self.exists;
        if self.loaded && near_end && has_more && self.more == More::Idle {
            self.more = More::Asked(self.envelopes.len());
            true
        } else {
            false
        }
    }

    pub(crate) fn find(&self, uid: u32) -> Option<&Envelope> {
        self.envelopes.iter().find(|envelope| envelope.uid == uid)
    }

    pub(crate) fn find_mut(&mut self, uid: u32) -> Option<&mut Envelope> {
        self.envelopes
            .iter_mut()
            .find(|envelope| envelope.uid == uid)
    }

    /// How many are Unread, of what's listed.
    pub(crate) fn unread(&self) -> usize {
        self.envelopes
            .iter()
            .filter(|envelope| !envelope.flags.seen)
            .count()
    }

    pub(crate) fn len(&self) -> usize {
        self.envelopes.len()
    }

    /// Takes Messages out, as moving them does, before the server says.
    pub(crate) fn remove(&mut self, uids: &[u32]) {
        self.envelopes
            .retain(|envelope| !uids.contains(&envelope.uid));
        self.selected.retain(|uid| !uids.contains(uid));
        self.arrange();
    }

    /// The UIDs of the rows, in the order they show.
    pub(crate) fn uids(&self) -> Vec<u32> {
        self.rows
            .iter()
            .map(|row| self.envelopes[row.envelope].uid)
            .collect()
    }

    /// The Message shown after `uid`, or before it if not `down`; `None` at either end.
    pub(crate) fn next(&self, uid: u32, down: bool) -> Option<u32> {
        let uids = self.uids();
        let at = uids.iter().position(|&listed| listed == uid)?;
        let next = if down { at + 1 } else { at.checked_sub(1)? };
        uids.get(next).copied()
    }

    /// Where Message `uid` is: the key of its thread's block, and its line there.
    pub(crate) fn place_of(&self, uid: u32) -> Option<(String, usize)> {
        let at = self
            .rows
            .iter()
            .position(|row| self.envelopes[row.envelope].uid == uid)?;
        let head = self.rows[at].head;
        let first = self.rows.iter().position(|row| row.head == head)?;
        Some((block_key(head), at - first))
    }

    /// The rows as blocks of a kit document, `width` cells wide: a block per thread, its rows its lines, with a fold
    /// over them whose summary is the head's row saying how many it holds. Or what's said while there are none.
    pub(crate) fn blocks(&self, width: usize, look: &Look) -> Vec<(BlockKey, Content<Target>)> {
        if !self.loaded {
            return vec![(
                BlockKey::new("waiting"),
                Content::from(vec![Line::Text(vec![Span::new(
                    format!(" Fetching {}…", self.label),
                    hl::DIM,
                )])]),
            )];
        }
        if self.rows.is_empty() {
            return vec![(
                BlockKey::new("empty"),
                Content::from(vec![Line::Text(vec![Span::new(" Nothing here.", hl::DIM)])]),
            )];
        }
        let mut blocks = Vec::new();
        let mut at = 0;
        while at < self.rows.len() {
            let head = self.rows[at].head;
            let end = self.rows[at..]
                .iter()
                .position(|row| row.head != head)
                .map_or(self.rows.len(), |len| at + len);
            blocks.push((
                BlockKey::new(block_key(head)),
                self.thread(&self.rows[at..end], width, look),
            ));
            at = end;
        }
        blocks
    }

    /// The rows of a thread, `width` cells wide, with a fold over them whose summary is the head's row saying how many
    /// it holds.
    fn thread(&self, thread: &[Row], width: usize, look: &Look) -> Content<Target> {
        let lines: Vec<Line> = thread
            .iter()
            .map(|row| self.line(row, None, width, look))
            .collect();
        let mut content = Content::from(lines);
        if thread.len() > 1 {
            let unread = thread
                .iter()
                .any(|row| !self.envelopes[row.envelope].flags.seen);
            content.folds.push(Fold {
                lines: 0..thread.len(),
                summary: self.line(&thread[0], Some((thread.len(), unread)), width, look),
                closed: false,
            });
        }
        content
    }

    /// The line of `row`, `width` cells wide; or of its thread folded into it, of `folded` Messages, any Unread.
    fn line(&self, row: &Row, folded: Option<(usize, bool)>, width: usize, look: &Look) -> Line {
        let envelope = &self.envelopes[row.envelope];
        let target = Target::Message(Opened {
            account: self.account,
            mailbox: self.mailbox.clone(),
            uid: envelope.uid,
        });
        let unread = !envelope.flags.seen || folded.is_some_and(|(_, unread)| unread);
        let text = if unread { hl::UNREAD } else { hl::READ };
        let mark = |on: bool, icon: &str, group: Group| {
            let shown = if on {
                icon.to_owned()
            } else {
                " ".repeat(icon.width())
            };
            Span::new(shown, group)
        };
        let selected = self.selected.contains(&envelope.uid);
        let mut spans = vec![
            Span::new(if selected { "▌" } else { " " }, hl::ACCENT),
            mark(unread, &look.unread, hl::ACCENT),
            mark(envelope.flags.flagged, &look.flagged, hl::FLAGGED),
            mark(envelope.flags.answered, &look.answered, hl::DIM),
            mark(envelope.has_attachments, &look.attachment, hl::ATTACHMENT),
        ];
        let flags_width: usize = spans.iter().map(|span| span.text.width()).sum();
        let date = envelope.date.map_or_else(String::new, |date| {
            short_date(&date.to_zoned(TimeZone::system()), &look.now)
        });
        let who = if self.sent {
            envelope
                .to
                .first()
                .map_or_else(String::new, |to| format!("To {}", to.short()))
        } else {
            envelope
                .from
                .first()
                .map_or_else(String::new, |from| from.short().to_owned())
        };
        let who_width = (width / 4).clamp(12, 28);
        spans.push(Span::new(format!("{date:>6} "), hl::DIM));
        let who = truncate(&who, who_width);
        let who_padding = who_width.saturating_sub(who.width());
        spans.push(Span::new(who, text));
        spans.push(Span::new(" ".repeat(who_padding + 2), Group::NORMAL));
        let mut room = width.saturating_sub(flags_width + 7 + who_width + 2);
        if row.repeated && folded.is_none() {
            // mutt's arrow, for the subject left out.
            let arrow = truncate(&format!("{}>", row.tree), room);
            room = room.saturating_sub(arrow.width() + 1);
            spans.push(Span::new(arrow, hl::DIM));
            spans.push(Span::new(" ", Group::NORMAL));
            spans.extend(labels(&envelope.labels, room));
        } else {
            if !row.tree.is_empty() {
                let tree = truncate(&row.tree, room);
                room = room.saturating_sub(tree.width() + 1);
                spans.push(Span::new(tree, hl::DIM));
                spans.push(Span::new(" ", Group::NORMAL));
            }
            if let Some((count, _)) = folded {
                let count = format!("({count}) ");
                room = room.saturating_sub(count.width());
                spans.push(Span::new(count, hl::ACCENT));
            }
            // As Gmail shows them: before the subject, but leaving it most of the room.
            let labels = labels(&envelope.labels, room / 2);
            room = room.saturating_sub(labels.iter().map(|span| span.text.width()).sum());
            spans.extend(labels);
            let subject = if envelope.subject.is_empty() {
                "(no subject)"
            } else {
                &envelope.subject
            };
            spans.push(Span::new(truncate(subject, room), text));
        }
        // To the window's edge, as wide as the window.
        let used: usize = spans.iter().map(|span| span.text.width()).sum();
        if used < width {
            spans.push(Span::new(" ".repeat(width - used), Group::NORMAL));
        }
        Line::Text(
            spans
                .into_iter()
                .map(|span| span.with_target(target.clone()))
                .collect(),
        )
    }
}

/// The key of the block of the thread Message `head` heads.
fn block_key(head: u32) -> String {
    format!("t{head}")
}

/// The Labels named `labels`, as many as fit in `room` cells, each padded and a space apart from what follows.
fn labels(labels: &[String], room: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut left = room;
    for name in labels {
        let label = format!(" {name} ");
        let width = label.width() + 1;
        if width > left {
            break;
        }
        left -= width;
        spans.push(Span::new(label, hl::LABEL));
        spans.push(Span::new(" ", Group::NORMAL));
    }
    spans
}

/// `subject` without its `Re:`s and `Fwd:`s, as a reply's is compared with what it replies to's.
fn base_subject(subject: &str) -> &str {
    let mut rest = subject.trim();
    loop {
        let lower = rest.to_ascii_lowercase();
        let stripped = ["re:", "fwd:", "fw:", "aw:", "sv:"]
            .iter()
            .find(|prefix| lower.starts_with(*prefix))
            .map(|prefix| rest[prefix.len()..].trim_start());
        match stripped {
            Some(after) => rest = after,
            None => return rest,
        }
    }
}

/// A date as short as says it: the time today, the day this year, the month before.
fn short_date(date: &Zoned, now: &Zoned) -> String {
    if date.date() == now.date() {
        date.strftime("%H:%M").to_string()
    } else if date.year() == now.year() {
        date.strftime("%b %d").to_string()
    } else {
        date.strftime("%Y-%m").to_string()
    }
}

/// `text` cut to `width` cells, with an ellipsis if it's cut.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut cut = String::new();
    for character in text.chars() {
        if cut.width() + unicode_width::UnicodeWidthChar::width(character).unwrap_or(0) + 1 > width
        {
            break;
        }
        cut.push(character);
    }
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use muttrs_mail::{Address, Flags};

    use super::*;

    fn envelope(uid: u32, day: i8, subject: &str, from: &str) -> Envelope {
        Envelope {
            uid,
            flags: Flags::default(),
            date: Some(
                date(2026, 11, day)
                    .at(9, 0, 0, 0)
                    .to_zoned(TimeZone::UTC)
                    .unwrap()
                    .timestamp(),
            ),
            from: vec![Address {
                name: Some(from.into()),
                email: format!("{}@example.com", from.to_lowercase()),
            }],
            to: Vec::new(),
            cc: Vec::new(),
            subject: subject.into(),
            message_id: Some(format!("{uid}@example.com")),
            in_reply_to: Vec::new(),
            references: Vec::new(),
            size: 0,
            has_attachments: false,
            gmail_thread: None,
            labels: Vec::new(),
        }
    }

    fn reply(uid: u32, day: i8, to: &[u32]) -> Envelope {
        let references: Vec<String> = to.iter().map(|uid| format!("{uid}@example.com")).collect();
        Envelope {
            in_reply_to: references.last().cloned().into_iter().collect(),
            references,
            ..envelope(uid, day, "Re: Lunch", "Bob")
        }
    }

    fn index(threaded: bool) -> Index {
        Index::new(
            0,
            "INBOX".into(),
            "Inbox".into(),
            Some(Role::Inbox),
            threaded,
        )
    }

    fn rows(index: &Index) -> Vec<(u32, String)> {
        index
            .rows
            .iter()
            .map(|row| (index.envelopes[row.envelope].uid, row.tree.clone()))
            .collect()
    }

    #[test]
    fn newest_first_and_next_in_order() {
        let mut index = index(false);
        index.set(
            vec![envelope(1, 1, "old", "Ann"), envelope(2, 2, "new", "Bob")],
            2,
        );
        assert_eq!(index.uids(), [2, 1]);
        assert_eq!(index.next(2, true), Some(1));
        assert_eq!(index.next(1, true), None, "nothing older");
        assert_eq!(index.next(1, false), Some(2));
    }

    #[test]
    fn threads_are_blocks_of_their_rows_folded_into_their_heads() {
        let mut index = index(true);
        index.set(
            vec![
                envelope(1, 1, "Lunch", "Ann"),
                reply(2, 2, &[1]),
                reply(3, 4, &[1, 2]),
                envelope(4, 3, "Report", "Cy"),
            ],
            4,
        );
        // The Lunch thread's latest reply (day 4) is newer than the Report (day 3).
        assert_eq!(
            rows(&index),
            [
                (1, String::new()),
                (2, "└─".into()),
                (3, "  └─".into()),
                (4, String::new())
            ]
        );
        assert_eq!(index.thread_of(2), [1, 2, 3]);
        assert_eq!(index.place_of(3), Some(("t1".to_owned(), 2)));
        assert_eq!(index.place_of(4), Some(("t4".to_owned(), 0)));
        let look = Look {
            unread: "● ".into(),
            flagged: "★ ".into(),
            answered: "↩ ".into(),
            attachment: "+ ".into(),
            now: Zoned::now(),
        };
        let blocks = index.blocks(100, &look);
        assert_eq!(blocks.len(), 2, "two threads");
        assert_eq!(blocks[0].1.lines.len(), 3);
        assert_eq!(blocks[0].1.folds[0].lines, 0..3);
        let Line::Text(folded) = &blocks[0].1.folds[0].summary else {
            panic!("a row of text");
        };
        let folded: String = folded.iter().map(|span| span.text.as_str()).collect();
        assert!(folded.contains("(3) Lunch"), "{folded:?}");
        assert!(folded.contains('●'), "Unread replies: {folded:?}");
        assert!(blocks[1].1.folds.is_empty(), "one Message: no fold");

        assert!(!index.toggle_threads());
        assert_eq!(
            rows(&index)
                .into_iter()
                .map(|(_, tree)| tree)
                .collect::<String>(),
            ""
        );
    }

    #[test]
    fn selecting_what_a_filter_picks() {
        let mut index = index(true);
        index.set(
            vec![
                envelope(1, 1, "Lunch", "Ann"),
                reply(2, 2, &[1]),
                envelope(3, 3, "Report", "Bob"),
            ],
            3,
        );
        assert_eq!(index.select_where(true, |e| e.uid != 1), (2, 2));
        assert_eq!(index.selection(), [2, 3]);
        assert_eq!(
            index.select_where(true, |_| true),
            (3, 1),
            "two were already"
        );
        assert_eq!(index.select_where(false, |e| e.uid == 3), (1, 1));
        assert_eq!(index.selection(), [1, 2]);
        index.toggle_selected(&[1, 2]);
        assert_eq!(index.selected(), 0);
        index.toggle_selected(&[3]);
        index.remove(&[3]);
        assert_eq!(index.selected(), 0, "what's gone isn't selected");
    }

    #[test]
    fn limited_only_what_the_filter_picks_shows_in_threads_of_its_own() {
        let mut index = index(true);
        index.set(
            vec![
                envelope(1, 1, "Lunch", "Ann"),
                reply(2, 2, &[1]),
                reply(3, 3, &[2]),
                envelope(4, 4, "Report", "Ann"),
            ],
            4,
        );
        let shown = index.limit(Some(("from:bob".into(), "from:bob".parse().unwrap())));
        assert_eq!(shown, 2);
        assert_eq!(
            rows(&index),
            [(2, String::new()), (3, "└─".to_owned())],
            "Bob's replies, a thread among themselves"
        );
        assert_eq!(index.limited(), Some("from:bob"));
        assert_eq!(
            index.select_where(true, |_| true),
            (2, 2),
            "only what shows"
        );
        assert_eq!(index.limit(None), 4);
        assert_eq!(rows(&index).len(), 4);
    }

    #[test]
    fn sorted_by_sender_subject_or_size_out_of_threads_or_the_other_way() {
        let mut index = index(true);
        let sized = |uid, day, subject: &str, from: &str, size| Envelope {
            size,
            ..envelope(uid, day, subject, from)
        };
        index.set(
            vec![
                sized(1, 1, "Re: banana", "carol", 30),
                sized(2, 2, "apple", "Bob", 10),
                sized(3, 3, "Cherry", "ann", 20),
                sized(4, 4, "apple", "Ann", 5),
            ],
            4,
        );
        index.sort(Some(Sort::Sender), false);
        assert_eq!(index.uids(), [4, 3, 2, 1], "ties newest first");
        assert_eq!(index.sorted(), Some("by sender"));
        index.sort(Some(Sort::Subject), false);
        assert_eq!(index.uids(), [4, 2, 1, 3], "without Re:");
        index.sort(Some(Sort::Size), true);
        assert_eq!(index.uids(), [4, 2, 3, 1]);
        assert_eq!(index.sorted(), Some("smallest first"));
        index.sort(Some(Sort::Date), true);
        assert_eq!(index.uids(), [1, 2, 3, 4], "oldest first");
        index.sort(None, false);
        assert!(index.threaded);
        assert_eq!(Sort::named("z"), Ok(Some(Sort::Size)));
        assert_eq!(Sort::named("threads"), Ok(None));
        assert!(Sort::named("colour").is_err());
    }

    #[test]
    fn older_envelopes_are_asked_for_once_near_the_end() {
        let mut index = index(false);
        let page = |count: u32| {
            (1..=count)
                .map(|uid| envelope(uid, 1, "s", "Ann"))
                .collect::<Vec<_>>()
        };
        index.set(page(40), 100);
        assert!(!index.wants_more(false), "the cursor is far from the end");
        assert!(index.wants_more(true));
        assert!(!index.wants_more(true), "asked already");
        index.set(page(80), 100);
        assert!(index.wants_more(true), "more came: ask again");
        index.set(page(80), 100);
        assert!(!index.wants_more(true), "none came: there are none");
    }

    #[test]
    fn base_subjects() {
        assert_eq!(base_subject("Re: RE: Fwd: Lunch"), "Lunch");
        assert_eq!(base_subject("Rebate"), "Rebate");
    }

    #[test]
    fn short_dates() {
        let now = date(2026, 11, 2)
            .at(15, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .unwrap();
        let at = |y, m, d| {
            date(y, m, d)
                .at(9, 5, 0, 0)
                .to_zoned(TimeZone::UTC)
                .unwrap()
        };
        assert_eq!(short_date(&at(2026, 11, 2), &now), "09:05");
        assert_eq!(short_date(&at(2026, 3, 14), &now), "Mar 14");
        assert_eq!(short_date(&at(2025, 12, 31), &now), "2025-12");
    }

    #[test]
    fn truncating_says_it_cut() {
        assert_eq!(truncate("Lunch on Thursday", 8), "Lunch o…");
        assert_eq!(truncate("Lunch", 8), "Lunch");
    }
}
