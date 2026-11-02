//! The Index: the Envelopes of the open Mailbox, newest first, one a row.

use jiff::Zoned;
use jiff::tz::TimeZone;
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

pub(crate) struct Index {
    /// The Account the Mailbox is of, by its place in the sidebar.
    pub(crate) account: usize,
    /// The Mailbox's name on the server.
    pub(crate) mailbox: String,
    /// The Mailbox's name for people.
    pub(crate) label: String,
    /// Whether the Mailbox holds what the user sent, so that rows show whom to rather than whom from.
    sent: bool,
    envelopes: Vec<Envelope>,
    /// How many Messages the Mailbox has, of which `envelopes` are the newest.
    pub(crate) exists: u32,
    /// Whether the Envelopes have come.
    pub(crate) loaded: bool,
    /// Where asking for older Envelopes stands.
    more: More,
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
    pub(crate) fn new(account: usize, mailbox: String, label: String, role: Option<Role>) -> Self {
        Self {
            account,
            mailbox,
            label,
            sent: matches!(role, Some(Role::Sent | Role::Drafts)),
            envelopes: Vec::new(),
            exists: 0,
            loaded: false,
            more: More::Idle,
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
        if let Some(uid) = at
            && let Some(row) = self
                .envelopes
                .iter()
                .position(|envelope| envelope.uid == uid)
        {
            self.cursor = row;
        }
        self.cursor = self.cursor.min(self.envelopes.len().saturating_sub(1));
        if let Some(search) = self.search.take() {
            self.search_for(search.query);
        }
    }

    /// Whether to ask for older Envelopes: the cursor is near the end of those listed, and the Mailbox has more.
    /// Asking is noted, so that it's asked once.
    pub(crate) fn wants_more(&mut self) -> bool {
        let near_end = self.cursor + MORE_WITHIN >= self.envelopes.len();
        let has_more = u32::try_from(self.envelopes.len()).unwrap_or(u32::MAX) < self.exists;
        if self.loaded && near_end && has_more && self.more == More::Idle {
            self.more = More::Asked(self.envelopes.len());
            true
        } else {
            false
        }
    }

    pub(crate) fn current(&self) -> Option<&Envelope> {
        self.envelopes.get(self.cursor)
    }

    pub(crate) fn current_mut(&mut self) -> Option<&mut Envelope> {
        self.envelopes.get_mut(self.cursor)
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
        self.cursor = self.cursor.min(self.envelopes.len().saturating_sub(1));
    }

    pub(crate) fn move_by(&mut self, rows: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(rows)
            .min(self.envelopes.len().saturating_sub(1));
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
        self.cursor = self.envelopes.len().saturating_sub(1);
    }

    /// Moves to the next Message, or the previous: what's the next one older, or newer. `false` if there's none.
    pub(crate) fn step(&mut self, older: bool) -> bool {
        let next = if older {
            self.cursor + 1
        } else {
            match self.cursor.checked_sub(1) {
                Some(previous) => previous,
                None => return false,
            }
        };
        if next < self.envelopes.len() {
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
            .envelopes
            .iter()
            .enumerate()
            .filter(|(_, envelope)| {
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
        if self.envelopes.is_empty() {
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
        for (row, envelope) in self
            .envelopes
            .iter()
            .enumerate()
            .skip(self.top)
            .take(self.height)
        {
            let y = area.y + u16::try_from(row - self.top).unwrap_or(0);
            let line = self.row(envelope, usize::from(area.width), &now, cx);
            buffer.set_line(area.x, y, &line, area.width);
            let row_area = Rect::new(area.x, y, area.width, 1);
            if row == self.cursor {
                let style = if cx.focused {
                    cx.theme.cursor
                } else {
                    cx.theme.cursor_unfocused
                };
                buffer.set_style(row_area, style);
            } else if matches.is_some_and(|matches| matches.contains(&row)) {
                buffer.set_style(Rect::new(area.x, y, 1, 1), cx.theme.found);
            }
        }
    }

    fn row(&self, envelope: &Envelope, width: usize, now: &Zoned, cx: &Draw<'_>) -> Line<'static> {
        let theme = cx.theme;
        let icons = cx.icons;
        let text = if envelope.flags.seen {
            theme.read
        } else {
            theme.unread
        };
        let mark = |on: bool, icon: &str, style| {
            let blank = " ".repeat(icon.width());
            Span::styled(if on { icon.to_owned() } else { blank }, style)
        };
        let flags = vec![
            Span::raw(" "),
            mark(!envelope.flags.seen, icons.unread, theme.accent),
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
        let subject_width = width.saturating_sub(flags_width + 7 + who_width + 2);
        let subject = if envelope.subject.is_empty() {
            "(no subject)"
        } else {
            &envelope.subject
        };
        let mut spans = flags;
        spans.push(Span::styled(format!("{date:>6} "), theme.dim));
        let who = truncate(&who, who_width);
        let who_padding = who_width.saturating_sub(who.width());
        spans.push(Span::styled(who, text));
        spans.push(Span::raw(" ".repeat(who_padding + 2)));
        spans.push(Span::styled(truncate(subject, subject_width), text));
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
            message_id: None,
            in_reply_to: Vec::new(),
            references: Vec::new(),
            size: 0,
            has_attachments: false,
        }
    }

    #[test]
    fn newest_first_and_the_cursor_stays_on_its_message() {
        let mut index = Index::new(0, "INBOX".into(), "Inbox".into(), Some(Role::Inbox));
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
    fn searching_senders_and_subjects() {
        let mut index = Index::new(0, "INBOX".into(), "Inbox".into(), None);
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
        let mut index = Index::new(0, "INBOX".into(), "Inbox".into(), None);
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
