//! The Index: the Envelopes of the open Mailbox, one a row, newest first, or in threads, each under what it
//! replies to and threads with the latest replies first. A thread can be folded into the row heading it.

use std::collections::HashSet;

use jiff::Zoned;
use jiff::tz::TimeZone;
use muttrs_mail::thread::{self, Branch};
use muttrs_mail::{Envelope, Role};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr as _;

use crate::icons::Icons;
use crate::theme::Theme;
use crate::ui::truncate;

/// How near the end of the Envelopes listed the cursor comes before older ones are asked for.
const MORE_WITHIN: usize = 30;

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
    /// How many replies are folded into it.
    folded: usize,
    /// Whether any of them is Unread.
    folded_unread: bool,
    /// Whether its subject shows: not when it's its thread head's, as a reply's usually is.
    subject: bool,
}

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
    /// The threads folded, by the UID of the Message heading them.
    folded: HashSet<u32>,
    /// How many Messages the Mailbox has, of which `envelopes` are the newest.
    pub(crate) exists: u32,
    /// Whether the Envelopes have come.
    pub(crate) loaded: bool,
    /// Where asking for older Envelopes stands.
    more: More,
    /// What a search on the server found, if that's what this lists: its query, and how many it found.
    pub(crate) found: Option<(String, usize)>,
    cursor: usize,
    top: usize,
    height: usize,
    search: Option<Search>,
}

struct Search {
    query: String,
    /// The rows that match, in order.
    matches: Vec<usize>,
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
            folded: HashSet::new(),
            exists: 0,
            loaded: false,
            more: More::Idle,
            found: None,
            cursor: 0,
            top: 0,
            height: 0,
            search: None,
        }
    }

    /// Takes the Envelopes the server listed, keeping the cursor on the Message it was on.
    pub(crate) fn set(&mut self, mut envelopes: Vec<Envelope>, exists: u32) {
        let at = self.current().map(|envelope| envelope.uid);
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
        self.arrange(at);
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
        found.set(envelopes, u32::try_from(count).unwrap_or(u32::MAX));
        found.more = More::Exhausted;
        found.found = Some((query, count));
        found
    }

    /// Lays the rows out again, the cursor on Message `at` if it still shows.
    fn arrange(&mut self, at: Option<u32>) {
        let row = self.cursor;
        self.rows = if self.threaded {
            self.threads()
        } else {
            (0..self.envelopes.len())
                .map(|envelope| Row {
                    envelope,
                    tree: String::new(),
                    head: self.envelopes[envelope].uid,
                    folded: 0,
                    folded_unread: false,
                    subject: true,
                })
                .collect()
        };
        self.cursor = at
            .and_then(|uid| self.position(uid))
            .unwrap_or(row)
            .min(self.rows.len().saturating_sub(1));
        if let Some(search) = self.search.take() {
            self.search_for(search.query);
        }
    }

    fn threads(&self) -> Vec<Row> {
        let threaded = thread::threads(&self.envelopes);
        let mut rows: Vec<Row> = Vec::with_capacity(threaded.len());
        let mut head_subject = "";
        let mut head = 0;
        for (at, message) in threaded.iter().enumerate() {
            let envelope = &self.envelopes[message.envelope];
            if message.depth == 0 {
                head = envelope.uid;
                head_subject = thread::base_subject(&envelope.subject);
                let folded = self.folded.contains(&head) && message.replies > 0;
                let replies = &threaded[at + 1..at + 1 + message.replies];
                let folded_unread = folded
                    && replies
                        .iter()
                        .any(|reply| !self.envelopes[reply.envelope].flags.seen);
                rows.push(Row {
                    envelope: message.envelope,
                    tree: String::new(),
                    head,
                    folded: if folded { message.replies } else { 0 },
                    folded_unread,
                    subject: true,
                });
            } else if !self.folded.contains(&head) {
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
                    envelope: message.envelope,
                    tree,
                    head,
                    folded: 0,
                    folded_unread: false,
                    subject: thread::base_subject(&envelope.subject) != head_subject,
                });
            }
        }
        rows
    }

    /// The row showing Message `uid`.
    fn position(&self, uid: u32) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| self.envelopes[row.envelope].uid == uid)
    }

    /// Shows rows in threads, or not.
    pub(crate) fn toggle_threads(&mut self) -> bool {
        let at = self.current().map(|envelope| envelope.uid);
        self.threaded = !self.threaded;
        self.arrange(at);
        self.threaded
    }

    /// Folds the thread at the cursor into its head's row, or unfolds it.
    pub(crate) fn toggle_fold(&mut self) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        let head = row.head;
        if !self.folded.remove(&head) {
            self.folded.insert(head);
        }
        self.arrange(Some(head));
    }

    /// Folds every thread, or unfolds them all.
    pub(crate) fn fold_all(&mut self, fold: bool) {
        let at = self.rows.get(self.cursor).map(|row| row.head);
        self.folded = if fold {
            self.rows.iter().map(|row| row.head).collect()
        } else {
            HashSet::new()
        };
        self.arrange(at);
    }

    /// Whether to ask for older Envelopes: the cursor is near the end of those listed, and the Mailbox has more.
    /// Asking is noted, so that it's asked once.
    pub(crate) fn wants_more(&mut self) -> bool {
        let near_end = self.cursor + MORE_WITHIN >= self.rows.len();
        let has_more = u32::try_from(self.envelopes.len()).unwrap_or(u32::MAX) < self.exists;
        if self.loaded && near_end && has_more && self.more == More::Idle {
            self.more = More::Asked(self.envelopes.len());
            true
        } else {
            false
        }
    }

    pub(crate) fn current(&self) -> Option<&Envelope> {
        let row = self.rows.get(self.cursor)?;
        self.envelopes.get(row.envelope)
    }

    pub(crate) fn current_mut(&mut self) -> Option<&mut Envelope> {
        let row = self.rows.get(self.cursor)?;
        self.envelopes.get_mut(row.envelope)
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
        // The cursor stays on its row: on what followed what's taken out.
        self.arrange(None);
    }

    pub(crate) fn move_by(&mut self, rows: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(rows)
            .min(self.rows.len().saturating_sub(1));
    }

    pub(crate) fn move_pages(&mut self, pages: f32) {
        // A page is at most a few thousand rows; rounding is what's wanted.
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let rows = (self.height.max(2) as f32 * pages).round() as isize;
        self.move_by(rows);
    }

    pub(crate) fn top(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
    }

    /// Moves to the next row, or the previous. `false` if there's none.
    pub(crate) fn step(&mut self, down: bool) -> bool {
        let next = if down {
            self.cursor + 1
        } else {
            match self.cursor.checked_sub(1) {
                Some(previous) => previous,
                None => return false,
            }
        };
        if next < self.rows.len() {
            self.cursor = next;
            true
        } else {
            false
        }
    }

    /// Searches the rows' senders and subjects for `query`, ignoring case, going to the first match from the cursor.
    pub(crate) fn search_for(&mut self, query: String) {
        let lower = query.to_lowercase();
        let matches: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let envelope = &self.envelopes[row.envelope];
                envelope.subject.to_lowercase().contains(&lower)
                    || envelope
                        .from
                        .iter()
                        .chain(&envelope.to)
                        .any(|address| address.to_string().to_lowercase().contains(&lower))
            })
            .map(|(row, _)| row)
            .collect();
        if let Some(&first) = matches
            .iter()
            .find(|&&row| row >= self.cursor)
            .or(matches.first())
        {
            self.cursor = first;
        }
        self.search = Some(Search { query, matches });
    }

    pub(crate) fn next_match(&mut self, forward: bool) {
        let Some(search) = &self.search else { return };
        let next = if forward {
            search
                .matches
                .iter()
                .find(|&&row| row > self.cursor)
                .or(search.matches.first())
        } else {
            search
                .matches
                .iter()
                .rev()
                .find(|&&row| row < self.cursor)
                .or(search.matches.last())
        };
        if let Some(&row) = next {
            self.cursor = row;
        }
    }

    pub(crate) fn clear_search(&mut self) -> bool {
        self.search.take().is_some()
    }

    /// The query searched for, which match the cursor is on (if any), and how many there are.
    pub(crate) fn search_status(&self) -> Option<(String, Option<usize>, usize)> {
        let search = self.search.as_ref()?;
        let at = search.matches.iter().position(|&row| row == self.cursor);
        Some((
            search.query.clone(),
            at.map(|at| at + 1),
            search.matches.len(),
        ))
    }

    pub(crate) fn draw(&mut self, area: Rect, buffer: &mut Buffer, cx: &Draw<'_>) {
        self.height = usize::from(area.height);
        if !self.loaded {
            let waiting = Line::styled(
                format!(" {}Fetching {}…", cx.icons.loading, self.label),
                cx.theme.dim,
            );
            buffer.set_line(area.x, area.y, &waiting, area.width);
            return;
        }
        if self.rows.is_empty() {
            buffer.set_line(
                area.x,
                area.y,
                &Line::styled(" Nothing here.", cx.theme.dim),
                area.width,
            );
            return;
        }
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.height > 0 && self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        let now = Zoned::now();
        let matches = self.search.as_ref().map(|search| search.matches.as_slice());
        for (index, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(self.top)
            .take(self.height)
        {
            let y = area.y + u16::try_from(index - self.top).unwrap_or(0);
            let line = self.line(row, usize::from(area.width), &now, cx);
            buffer.set_line(area.x, y, &line, area.width);
            let row_area = Rect::new(area.x, y, area.width, 1);
            if index == self.cursor {
                let style = if cx.focused {
                    cx.theme.cursor
                } else {
                    cx.theme.cursor_unfocused
                };
                buffer.set_style(row_area, style);
            } else if matches.is_some_and(|matches| matches.contains(&index)) {
                buffer.set_style(Rect::new(area.x, y, 1, 1), cx.theme.found);
            }
        }
    }

    fn line(&self, row: &Row, width: usize, now: &Zoned, cx: &Draw<'_>) -> Line<'static> {
        let envelope = &self.envelopes[row.envelope];
        let theme = cx.theme;
        let icons = cx.icons;
        let unread = !envelope.flags.seen || row.folded_unread;
        let text = if unread { theme.unread } else { theme.read };
        let mark = |on: bool, icon: &str, style| {
            let blank = " ".repeat(icon.width());
            Span::styled(if on { icon.to_owned() } else { blank }, style)
        };
        let flags = vec![
            Span::raw(" "),
            mark(unread, icons.unread, theme.accent),
            mark(envelope.flags.flagged, icons.flagged, theme.flagged),
            mark(envelope.flags.answered, icons.answered, theme.dim),
            mark(envelope.has_attachments, icons.attachment, theme.attachment),
        ];
        let flags_width: usize = flags.iter().map(|span| span.content.width()).sum();
        let date = envelope.date.map_or_else(String::new, |date| {
            short_date(&date.to_zoned(TimeZone::system()), now)
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
        let mut spans = flags;
        spans.push(Span::styled(format!("{date:>6} "), theme.dim));
        let who = truncate(&who, who_width);
        let who_padding = who_width.saturating_sub(who.width());
        spans.push(Span::styled(who, text));
        spans.push(Span::raw(" ".repeat(who_padding + 2)));
        let mut room = width.saturating_sub(flags_width + 7 + who_width + 2);
        if !row.tree.is_empty() {
            let tree = truncate(&row.tree, room);
            room = room.saturating_sub(tree.width() + 1);
            spans.push(Span::styled(tree, theme.dim));
            spans.push(Span::raw(" "));
        }
        if row.folded > 0 {
            let count = format!("({}) ", row.folded + 1);
            room = room.saturating_sub(count.width());
            spans.push(Span::styled(count, theme.accent));
        }
        if row.subject {
            let subject = if envelope.subject.is_empty() {
                "(no subject)"
            } else {
                &envelope.subject
            };
            spans.push(Span::styled(truncate(subject, room), text));
        }
        Line::from(spans)
    }
}

/// What drawing the Index takes.
pub(crate) struct Draw<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) icons: &'a Icons,
    pub(crate) focused: bool,
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

    #[test]
    fn newest_first_and_the_cursor_stays_on_its_message() {
        let mut index = index(false);
        index.set(
            vec![envelope(1, 1, "old", "Ann"), envelope(2, 2, "new", "Bob")],
            2,
        );
        assert_eq!(index.current().unwrap().subject, "new");
        index.move_by(1);
        assert_eq!(index.current().unwrap().subject, "old");
        index.set(
            vec![
                envelope(1, 1, "old", "Ann"),
                envelope(2, 2, "new", "Bob"),
                envelope(3, 3, "newer", "Cy"),
            ],
            3,
        );
        assert_eq!(
            index.current().unwrap().subject,
            "old",
            "still on it, a row further down"
        );
        assert!(!index.step(true), "nothing older");
        assert!(index.step(false));
        assert_eq!(index.current().unwrap().subject, "new");
    }

    #[test]
    fn threads_fold_into_their_heads() {
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
        let shown = |index: &Index| -> Vec<(u32, String, bool)> {
            index
                .rows
                .iter()
                .map(|row| {
                    (
                        index.envelopes[row.envelope].uid,
                        row.tree.clone(),
                        row.subject,
                    )
                })
                .collect()
        };
        // The Lunch thread's latest reply (day 4) is newer than the Report (day 3).
        assert_eq!(
            shown(&index),
            [
                (1, String::new(), true),
                (2, "└─".into(), false),
                (3, "  └─".into(), false),
                (4, String::new(), true)
            ]
        );
        index.move_by(2);
        index.toggle_fold();
        assert_eq!(shown(&index).len(), 2);
        assert_eq!(
            index.current().unwrap().uid,
            1,
            "the cursor goes to the thread's head"
        );
        assert_eq!(index.rows[0].folded, 2);
        assert!(index.rows[0].folded_unread);
        index.fold_all(false);
        assert_eq!(shown(&index).len(), 4);

        assert!(!index.toggle_threads());
        assert_eq!(
            index.current().unwrap().uid,
            1,
            "the cursor stays on its Message"
        );
        assert_eq!(
            index
                .rows
                .iter()
                .map(|row| row.tree.as_str())
                .collect::<String>(),
            ""
        );
    }

    #[test]
    fn searching_senders_and_subjects() {
        let mut index = index(false);
        index.set(
            vec![
                envelope(1, 1, "Lunch", "Ann"),
                envelope(2, 2, "Report", "Bob"),
                envelope(3, 3, "lunch?", "Cy"),
            ],
            3,
        );
        index.search_for("LUNCH".into());
        assert_eq!(index.current().unwrap().uid, 3);
        index.next_match(true);
        assert_eq!(index.current().unwrap().uid, 1);
        assert_eq!(index.search_status(), Some(("LUNCH".into(), Some(2), 2)));
        index.search_for("bob@".into());
        assert_eq!(index.current().unwrap().uid, 2);
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
        assert!(!index.wants_more(), "the cursor is far from the end");
        index.bottom();
        assert!(index.wants_more());
        assert!(!index.wants_more(), "asked already");
        index.set(page(80), 100);
        assert!(index.wants_more(), "more came: ask again");
        index.set(page(80), 100);
        assert!(!index.wants_more(), "none came: there are none");
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
}
