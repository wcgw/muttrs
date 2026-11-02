//! Lists that pop up over what's on screen: a Message's Links, its Attachments, the keys.

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
            Kind::Help => " Esc ",
        };
        Self {
            kind,
            title: title.into(),
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

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let count = self.rows.len();
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
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
            .title(Span::styled(format!(" {} ", self.title), theme.accent))
            .title_bottom(Line::styled(self.hint, theme.dim).right_aligned());
        let inner = block.inner(popup);
        block.render(popup, buffer);

        let visible = usize::from(inner.height);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if visible > 0 && self.cursor >= self.top + visible {
            self.top = self.cursor + 1 - visible;
        }
        for (index, row) in self.rows.iter().enumerate().skip(self.top).take(visible) {
            let y = inner.y + u16::try_from(index - self.top).unwrap_or(0);
            buffer.set_line(inner.x + 1, y, row, inner.width.saturating_sub(2));
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
}
