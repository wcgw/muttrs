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

use crate::filter::Filter;
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
    /// The UIDs of the replies folded into it, which what's done to it is done to.
    folded_uids: Vec<u32>,
    /// Whether any of them is Unread.
    folded_unread: bool,
    /// Whether its subject is what it replies to's, and isn't shown again: an arrow shows instead.
    repeated: bool,
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
    /// The threads folded, by the UID of the Message heading them.
    folded: HashSet<u32>,
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
    cursor: usize,
    top: usize,
    height: usize,
    /// The first row drawn around the cursor, as above the Pager.
    around: usize,
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
            hide_subjects: false,
            folded: HashSet::new(),
            selected: HashSet::new(),
            exists: 0,
            loaded: false,
            more: More::Idle,
            found: None,
            limit: None,
            cursor: 0,
            top: 0,
            height: 0,
            around: 0,
            search: None,
        }
    }

    /// The Index, leaving a reply's subject out where it's what it replies to's, if `hide`.
    pub(crate) fn hiding_repeated_subjects(mut self, hide: bool) -> Self {
        self.hide_subjects = hide;
        self
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
        found.hide_subjects = self.hide_subjects;
        found.set(envelopes, u32::try_from(count).unwrap_or(u32::MAX));
        found.more = More::Exhausted;
        found.found = Some((query, count));
        found
    }

    /// Lays the rows out again, the cursor on Message `at` if it still shows.
    fn arrange(&mut self, at: Option<u32>) {
        let row = self.cursor;
        let shown: Vec<usize> = (0..self.envelopes.len())
            .filter(|&envelope| self.within_limit(&self.envelopes[envelope]))
            .collect();
        self.rows = if self.threaded {
            self.threads(&shown)
        } else {
            shown
                .into_iter()
                .map(|envelope| Row {
                    envelope,
                    tree: String::new(),
                    head: self.envelopes[envelope].uid,
                    folded: 0,
                    folded_uids: Vec::new(),
                    folded_unread: false,
                    repeated: false,
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
        for (at, message) in threaded.iter().enumerate() {
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
                let folded = self.folded.contains(&head) && message.replies > 0;
                let replies = &threaded[at + 1..at + 1 + message.replies];
                let folded_unread = folded
                    && replies
                        .iter()
                        .any(|reply| !envelopes[reply.envelope].flags.seen);
                rows.push(Row {
                    envelope: place(message.envelope),
                    tree: String::new(),
                    head,
                    folded: if folded { message.replies } else { 0 },
                    folded_uids: if folded {
                        replies
                            .iter()
                            .map(|reply| envelopes[reply.envelope].uid)
                            .collect()
                    } else {
                        Vec::new()
                    },
                    folded_unread,
                    repeated: false,
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
                    envelope: place(message.envelope),
                    tree,
                    head,
                    folded: 0,
                    folded_uids: Vec::new(),
                    folded_unread: false,
                    repeated,
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

    /// Selects the Message at the cursor (and the replies folded into it), or unselects it, and moves down.
    pub(crate) fn toggle_selected(&mut self) {
        let uids = self.at_cursor();
        let Some(first) = uids.first() else { return };
        if self.selected.contains(first) {
            for uid in &uids {
                self.selected.remove(uid);
            }
        } else {
            self.selected.extend(uids);
        }
        self.move_by(1);
    }

    /// Unselects everything; `false` if nothing was.
    pub(crate) fn clear_selected(&mut self) -> bool {
        let any = !self.selected.is_empty();
        self.selected.clear();
        any
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
        let at = self.current().map(|envelope| envelope.uid);
        self.limit = limit;
        self.arrange(at);
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

    /// The UIDs of the Message at the cursor and of the replies folded into it.
    fn at_cursor(&self) -> Vec<u32> {
        let Some(row) = self.rows.get(self.cursor) else {
            return Vec::new();
        };
        std::iter::once(self.envelopes[row.envelope].uid)
            .chain(row.folded_uids.iter().copied())
            .collect()
    }

    /// The UIDs of the Messages an action is for: those selected, or else the one at the cursor (with the replies
    /// folded into it).
    pub(crate) fn targets(&self) -> Vec<u32> {
        if self.selected.is_empty() {
            self.at_cursor()
        } else {
            let mut uids: Vec<u32> = self.selected.iter().copied().collect();
            uids.sort_unstable();
            uids
        }
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

    /// Puts the cursor on the row drawn `offset` rows down from the top of the Index; `false` if it was there
    /// already, as a second click is, or there's no row there.
    pub(crate) fn click(&mut self, offset: usize) -> bool {
        self.put_cursor(self.top + offset)
    }

    /// As [`Index::click`], on the rows drawn around the cursor.
    pub(crate) fn click_around_cursor(&mut self, offset: usize) -> bool {
        self.put_cursor(self.around + offset)
    }

    fn put_cursor(&mut self, row: usize) -> bool {
        if row >= self.rows.len() || row == self.cursor {
            return false;
        }
        self.cursor = row;
        true
    }

    /// Whether the row drawn `offset` rows down from the top is the cursor's.
    pub(crate) fn is_cursor_at(&self, offset: usize) -> bool {
        self.top + offset == self.cursor && self.cursor < self.rows.len()
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

    /// Draws the rows around the cursor's, it a third of the way down (unless near the top or the end), as mutt's
    /// mini-index above the Pager, leaving where the Index itself is scrolled to as it was.
    pub(crate) fn draw_around_cursor(&mut self, area: Rect, buffer: &mut Buffer, cx: &Draw<'_>) {
        let height = usize::from(area.height);
        self.around = self
            .cursor
            .saturating_sub(height / 3)
            .min(self.rows.len().saturating_sub(height));
        self.draw_rows(self.around, area, buffer, cx);
    }

    pub(crate) fn draw(&mut self, area: Rect, buffer: &mut Buffer, cx: &Draw<'_>) {
        self.height = usize::from(area.height);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.height > 0 && self.cursor >= self.top + self.height {
            self.top = self.cursor + 1 - self.height;
        }
        self.draw_rows(self.top, area, buffer, cx);
    }

    /// Draws the rows from `top` down, in `area`.
    fn draw_rows(&self, top: usize, area: Rect, buffer: &mut Buffer, cx: &Draw<'_>) {
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
        let now = Zoned::now();
        let matches = self.search.as_ref().map(|search| search.matches.as_slice());
        for (index, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(top)
            .take(usize::from(area.height))
        {
            let y = area.y + u16::try_from(index - top).unwrap_or(0);
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
            if self.selected.contains(&self.envelopes[row.envelope].uid) {
                buffer.set_string(area.x, y, "▌", cx.theme.accent);
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
        if row.repeated {
            // mutt's arrow, for the subject left out.
            let arrow = truncate(&format!("{}>", row.tree), room);
            room = room.saturating_sub(arrow.width() + 1);
            spans.push(Span::styled(arrow, theme.dim));
            spans.push(Span::raw(" "));
            spans.extend(labels(&envelope.labels, room, theme));
            return Line::from(spans);
        }
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
        // As Gmail shows them: before the subject, but leaving it most of the room.
        let labels = labels(&envelope.labels, room / 2, theme);
        room = room.saturating_sub(labels.iter().map(|span| span.content.width()).sum());
        spans.extend(labels);
        let subject = if envelope.subject.is_empty() {
            "(no subject)"
        } else {
            &envelope.subject
        };
        spans.push(Span::styled(truncate(subject, room), text));
        Line::from(spans)
    }
}

/// The Labels named `labels`, as many as fit in `room` cells, each padded in `theme`'s label style and a space apart
/// from what follows.
fn labels(labels: &[String], room: usize, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut left = room;
    for name in labels {
        let label = format!(" {name} ");
        let width = label.width() + 1;
        if width > left {
            break;
        }
        left -= width;
        spans.push(Span::styled(label, theme.label));
        spans.push(Span::raw(" "));
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
        let shown = |index: &Index| -> Vec<(u32, String)> {
            index
                .rows
                .iter()
                .map(|row| (index.envelopes[row.envelope].uid, row.tree.clone()))
                .collect()
        };
        // The Lunch thread's latest reply (day 4) is newer than the Report (day 3).
        assert_eq!(
            shown(&index),
            [
                (1, String::new()),
                (2, "└─".into()),
                (3, "  └─".into()),
                (4, String::new())
            ]
        );

        // Every row says its subject, replies' too, the same as their thread's head's though it is.
        let theme = crate::theme::Theme::named("terminal", &Icons::PLAIN).unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 4));
        index.draw(
            buffer.area,
            &mut buffer,
            &Draw {
                theme: &theme,
                icons: &Icons::PLAIN,
                focused: true,
            },
        );
        let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(row(0).trim_end().ends_with(" Lunch"), "{:?}", row(0));
        assert!(row(1).trim_end().ends_with("└─ Re: Lunch"), "{:?}", row(1));
        assert!(
            row(2).trim_end().ends_with("  └─ Re: Lunch"),
            "{:?}",
            row(2)
        );

        // Hidden as mutt hides them, a reply's subject that's what it replies to's shows as an arrow.
        let mut hiding = Index::new(0, "INBOX".into(), "Inbox".into(), None, true)
            .hiding_repeated_subjects(true);
        hiding.set(
            vec![
                envelope(1, 1, "[PATCH 0/2] fix", "Ann"),
                Envelope {
                    subject: "[PATCH 1/2] first".into(),
                    ..reply(2, 2, &[1])
                },
                Envelope {
                    subject: "Re: [PATCH 1/2] first".into(),
                    ..reply(3, 3, &[1, 2])
                },
                Envelope {
                    subject: "RE: [patch 1/2] FIRST".into(),
                    ..reply(4, 4, &[1, 2, 3])
                },
            ],
            4,
        );
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 4));
        hiding.draw(
            buffer.area,
            &mut buffer,
            &Draw {
                theme: &theme,
                icons: &Icons::PLAIN,
                focused: true,
            },
        );
        let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(
            row(1).trim_end().ends_with("└─ [PATCH 1/2] first"),
            "not its parent's: {:?}",
            row(1)
        );
        assert!(
            row(2).trim_end().ends_with("  └─>"),
            "its parent's: {:?}",
            row(2)
        );
        assert!(
            row(3).trim_end().ends_with("    └─>"),
            "its parent's, case aside: {:?}",
            row(3)
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
        assert_eq!(
            index.targets(),
            [1, 2, 3],
            "what's done to a folded thread is done to it all"
        );
        index.fold_all(false);
        assert_eq!(shown(&index).len(), 4);
        assert_eq!(index.targets(), [1]);
        index.toggle_selected();
        index.toggle_selected();
        assert_eq!(
            (index.targets(), index.current().unwrap().uid),
            (vec![1, 2], 3)
        );
        index.remove(&[2]);
        assert_eq!(index.targets(), [1], "what's gone isn't selected");
        assert!(index.clear_selected());

        let at = index.current().unwrap().uid;
        assert!(!index.toggle_threads());
        assert_eq!(
            index.current().unwrap().uid,
            at,
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
    fn around_the_cursor_it_a_third_of_the_way_down() {
        let mut index = Index::new(0, "INBOX".into(), "Inbox".into(), None, false);
        let day = |uid: u32| i8::try_from(uid).unwrap();
        index.set(
            (1..=10)
                .map(|uid| envelope(uid, day(uid), &format!("m{uid}"), "Ann"))
                .collect(),
            10,
        );
        let theme = crate::theme::Theme::named("terminal", &Icons::PLAIN).unwrap();
        // The subjects of the 6 rows drawn, newest first: m10 the first row.
        let drawn = |index: &mut Index| -> Vec<String> {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 6));
            index.draw_around_cursor(
                buffer.area,
                &mut buffer,
                &Draw {
                    theme: &theme,
                    icons: &Icons::PLAIN,
                    focused: false,
                },
            );
            (0..6)
                .map(|y| {
                    let row: String = (0..60).map(|x| buffer[(x, y)].symbol()).collect();
                    row.split_whitespace().last().unwrap_or_default().to_owned()
                })
                .collect()
        };
        assert_eq!(
            drawn(&mut index),
            ["m10", "m9", "m8", "m7", "m6", "m5"],
            "near the top"
        );
        index.move_by(5);
        assert_eq!(
            drawn(&mut index),
            ["m7", "m6", "m5", "m4", "m3", "m2"],
            "m5, the cursor's, on the third row"
        );
        assert!(index.click_around_cursor(0));
        assert_eq!(
            index.current().map(|e| e.uid),
            Some(7),
            "the row drawn first"
        );
        index.move_by(6);
        assert_eq!(
            drawn(&mut index),
            ["m6", "m5", "m4", "m3", "m2", "m1"],
            "near the end"
        );

        // The Index itself, drawn whole again, is scrolled as it was, though it was drawn from m9 around m7.
        index.move_by(-6);
        assert_eq!(drawn(&mut index)[0], "m9");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 10));
        index.draw(
            buffer.area,
            &mut buffer,
            &Draw {
                theme: &theme,
                icons: &Icons::PLAIN,
                focused: true,
            },
        );
        let first: String = (0..60).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(first.trim_end().ends_with("m10"), "{first:?}");
    }

    #[test]
    fn selecting_what_a_filter_picks_folded_or_not() {
        let mut index = Index::new(0, "INBOX".into(), "Inbox".into(), None, true);
        index.set(
            vec![
                envelope(1, 1, "Lunch", "Ann"),
                reply(2, 2, &[1]),
                envelope(3, 3, "Report", "Bob"),
            ],
            3,
        );
        index.fold_all(true);
        assert_eq!(
            index.select_where(true, |e| e.uid != 1),
            (2, 2),
            "the folded reply too"
        );
        assert_eq!(index.targets(), [2, 3]);
        assert_eq!(
            index.select_where(true, |_| true),
            (3, 1),
            "two were already"
        );
        assert_eq!(index.select_where(false, |e| e.uid == 3), (1, 1));
        assert_eq!(index.targets(), [1, 2]);
        assert_eq!(
            index.select_where(false, |e| e.uid == 3),
            (1, 0),
            "not selected now"
        );
        assert_eq!(index.select_where(true, |_| false), (0, 0));
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
        let rows = |index: &Index| -> Vec<(u32, String)> {
            index
                .rows
                .iter()
                .map(|row| (index.envelopes[row.envelope].uid, row.tree.clone()))
                .collect()
        };
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
        assert_eq!(index.limited(), None);
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
}
