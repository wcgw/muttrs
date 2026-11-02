//! Lists that pop up over what's on screen: a Message's Links, its Attachments, the Mailboxes to move to, the
//! keys.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget as _};
use unicode_width::UnicodeWidthStr as _;

use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Links,
    Attachments,
    /// The Mailboxes to move Messages to, narrowed as their names are typed.
    Mailboxes,
    Help,
}

/// What's done to a row of a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verb {
    /// `Enter` or `o`: open it.
    Open,
    /// `y`: copy it.
    Copy,
    /// `s`: save it.
    Save,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Stay open.
    Stay,
    Close,
    /// Do this to the row at this index.
    Chosen(usize, Verb),
}

pub(crate) struct Overlay {
    kind: Kind,
    title: String,
    rows: Vec<Line<'static>>,
    /// What each row is found by, in lower case, where typing narrows the list.
    names: Vec<String>,
    /// What's typed to narrow the list.
    filter: String,
    /// The rows listed, narrowed by `filter`, by their index in `rows`.
    shown: Vec<usize>,
    /// The keys it takes, shown at its foot.
    hint: &'static str,
    cursor: usize,
    /// The row number typed so far.
    typed: String,
    /// The first row showing.
    top: usize,
}

impl Overlay {
    pub(crate) fn new(kind: Kind, title: impl Into<String>, rows: Vec<Line<'static>>) -> Self {
        let hint = match kind {
            Kind::Links => " 1-9 Enter open · y copy · Esc ",
            Kind::Attachments => " Enter open · s save · Esc ",
            Kind::Mailboxes => " type to narrow · ↑ ↓ Enter move · Esc ",
            Kind::Help => " Esc ",
        };
        let names = rows
            .iter()
            .map(|row| {
                row.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .to_lowercase()
            })
            .collect();
        Self {
            kind,
            title: title.into(),
            shown: (0..rows.len()).collect(),
            names,
            filter: String::new(),
            rows,
            hint,
            cursor: 0,
            typed: String::new(),
            top: 0,
        }
    }

    pub(crate) fn kind(&self) -> Kind {
        self.kind
    }

    /// Narrows the list to the rows whose names hold what's typed.
    fn narrow(&mut self) {
        let filter = self.filter.to_lowercase();
        self.shown = (0..self.rows.len())
            .filter(|&row| self.names[row].contains(&filter))
            .collect();
        self.cursor = self.cursor.min(self.shown.len().saturating_sub(1));
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.kind == Kind::Mailboxes {
            match key.code {
                KeyCode::Esc => return Outcome::Close,
                KeyCode::Char('c') if control => return Outcome::Close,
                KeyCode::Down | KeyCode::Tab => {
                    self.cursor = (self.cursor + 1).min(self.shown.len().saturating_sub(1));
                }
                KeyCode::Char('n') if control => {
                    self.cursor = (self.cursor + 1).min(self.shown.len().saturating_sub(1));
                }
                KeyCode::Up | KeyCode::BackTab => self.cursor = self.cursor.saturating_sub(1),
                KeyCode::Char('p') if control => self.cursor = self.cursor.saturating_sub(1),
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.narrow();
                }
                KeyCode::Char('u') if control => {
                    self.filter.clear();
                    self.narrow();
                }
                KeyCode::Char(character) => {
                    self.filter.push(character);
                    self.narrow();
                }
                KeyCode::Enter => {
                    if let Some(&row) = self.shown.get(self.cursor) {
                        return Outcome::Chosen(row, Verb::Open);
                    }
                }
                _ => {}
            }
            return Outcome::Stay;
        }
        let count = self.rows.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Close,
            KeyCode::Char('c') if control => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(count.saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => self.cursor = count.saturating_sub(1),
            _ if self.kind == Kind::Help => {}
            KeyCode::Char(digit @ '0'..='9') => {
                self.typed.push(digit);
                let number: usize = self.typed.parse().unwrap_or(0);
                if (1..=count).contains(&number) {
                    self.cursor = number - 1;
                    // Chosen once no more digits could make another row's number.
                    if number * 10 > count {
                        self.typed.clear();
                        return Outcome::Chosen(self.cursor, Verb::Open);
                    }
                } else {
                    self.typed.clear();
                }
                return Outcome::Stay;
            }
            KeyCode::Enter | KeyCode::Char('o') if count > 0 => {
                return Outcome::Chosen(self.cursor, Verb::Open);
            }
            KeyCode::Char('y') if count > 0 && self.kind == Kind::Links => {
                return Outcome::Chosen(self.cursor, Verb::Copy);
            }
            KeyCode::Char('s') if count > 0 && self.kind == Kind::Attachments => {
                return Outcome::Chosen(self.cursor, Verb::Save);
            }
            _ => {}
        }
        self.typed.clear();
        Outcome::Stay
    }

    pub(crate) fn draw(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let widest = self.rows.iter().map(Line::width).max().unwrap_or(0);
        let width = (widest + 4)
            .max(self.title.width() + 6)
            .max(self.hint.width() + 4)
            .min(usize::from(area.width.saturating_sub(4)));
        // A list narrowed by typing keeps its size, rather than shrink under the typing.
        let height = (self.rows.len() + 2).min(usize::from(area.height.saturating_sub(2)));
        // Both within the area's u16s, by the mins.
        let (width, height) = (
            u16::try_from(width).unwrap_or(area.width),
            u16::try_from(height).unwrap_or(area.height),
        );
        let popup = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        );
        Clear.render(popup, buffer);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.accent)
            .title(Span::styled(
                if self.kind == Kind::Mailboxes {
                    format!(" {}: {}▏", self.title, self.filter)
                } else {
                    format!(" {} ", self.title)
                },
                theme.accent,
            ))
            .title_bottom(Line::styled(self.hint, theme.dim).right_aligned());
        let inner = block.inner(popup);
        block.render(popup, buffer);

        let visible = usize::from(inner.height);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if visible > 0 && self.cursor >= self.top + visible {
            self.top = self.cursor + 1 - visible;
        }
        for (index, &row) in self.shown.iter().enumerate().skip(self.top).take(visible) {
            let y = inner.y + u16::try_from(index - self.top).unwrap_or(0);
            buffer.set_line(
                inner.x + 1,
                y,
                &self.rows[row],
                inner.width.saturating_sub(2),
            );
            if index == self.cursor && self.kind != Kind::Help {
                buffer.set_style(Rect::new(inner.x, y, inner.width, 1), theme.cursor);
            }
        }
    }
}

/// The help overlay: the keys, by where they work.
pub(crate) fn help(theme: &Theme, sections: &[(&str, &[(&str, &str)])]) -> Overlay {
    let mut rows = Vec::new();
    for (index, (section, keys)) in sections.iter().enumerate() {
        if index > 0 {
            rows.push(Line::default());
        }
        rows.push(Line::styled((*section).to_owned(), theme.dim));
        let widest = keys.iter().map(|(keys, _)| keys.width()).max().unwrap_or(0);
        for (keys, what) in *keys {
            rows.push(Line::from(vec![
                Span::styled(format!("{keys:<widest$}  "), theme.keys),
                Span::raw((*what).to_owned()),
            ]));
        }
    }
    Overlay::new(Kind::Help, "Keys", rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(overlay: &mut Overlay, code: KeyCode) -> Outcome {
        overlay.key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn a_number_chooses_once_no_other_could_follow() {
        let rows = (1..=12).map(|n| Line::raw(n.to_string())).collect();
        let mut links = Overlay::new(Kind::Links, "Links", rows);
        assert_eq!(
            press(&mut links, KeyCode::Char('1')),
            Outcome::Stay,
            "10, 11, 12 could follow"
        );
        assert_eq!(
            press(&mut links, KeyCode::Char('2')),
            Outcome::Chosen(11, Verb::Open)
        );
        assert_eq!(
            press(&mut links, KeyCode::Char('7')),
            Outcome::Chosen(6, Verb::Open)
        );
        assert_eq!(press(&mut links, KeyCode::Char('1')), Outcome::Stay);
        assert_eq!(
            press(&mut links, KeyCode::Enter),
            Outcome::Chosen(0, Verb::Open)
        );
        assert_eq!(
            press(&mut links, KeyCode::Char('y')),
            Outcome::Chosen(0, Verb::Copy)
        );
        assert_eq!(press(&mut links, KeyCode::Esc), Outcome::Close);
    }

    #[test]
    fn mailboxes_narrow_as_their_names_are_typed() {
        let rows = ["Inbox", "Archive", "Work/Reports", "Receipts"]
            .into_iter()
            .map(Line::raw)
            .collect();
        let mut mailboxes = Overlay::new(Kind::Mailboxes, "Move to", rows);
        for character in "re".chars() {
            assert_eq!(
                press(&mut mailboxes, KeyCode::Char(character)),
                Outcome::Stay
            );
        }
        // "re" is in Work/Reports and Receipts.
        assert_eq!(press(&mut mailboxes, KeyCode::Down), Outcome::Stay);
        assert_eq!(
            press(&mut mailboxes, KeyCode::Enter),
            Outcome::Chosen(3, Verb::Open)
        );
        press(&mut mailboxes, KeyCode::Char('x'));
        assert_eq!(
            press(&mut mailboxes, KeyCode::Enter),
            Outcome::Stay,
            "nothing's listed"
        );
        press(&mut mailboxes, KeyCode::Backspace);
        press(&mut mailboxes, KeyCode::Up);
        assert_eq!(
            press(&mut mailboxes, KeyCode::Enter),
            Outcome::Chosen(2, Verb::Open)
        );
    }
}
