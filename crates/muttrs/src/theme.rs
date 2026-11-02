//! muttrs' colours. Two are built in: Tokyo Night Moon (folke/tokyonight.nvim's `moon`), as ghd and slackers have,
//! and the terminal's own colours with emphasis only. Only text and highlights are coloured: everywhere else the
//! terminal's background shows.

use muttrs_render::{Glyphs, Rgb, Styles};
use ratatui::style::{Color, Modifier, Style};

use crate::icons::Icons;

/// Tokyo Night Moon's palette, by its own names.
mod moon {
    use ratatui::style::Color;

    pub(super) const BG: (u8, u8, u8) = (0x22, 0x24, 0x36);
    pub(super) const FG: Color = Color::Rgb(0xc8, 0xd3, 0xf5);
    pub(super) const FG_DARK: Color = Color::Rgb(0x82, 0x8b, 0xb8);
    pub(super) const COMMENT: Color = Color::Rgb(0x63, 0x6d, 0xa6);
    pub(super) const BG_HIGHLIGHT: Color = Color::Rgb(0x2f, 0x33, 0x4d);
    /// `blue0` blended 40% over `bg`, as tokyonight computes `bg_visual`.
    pub(super) const BG_VISUAL: Color = Color::Rgb(0x2d, 0x3f, 0x76);
    pub(super) const BLUE: Color = Color::Rgb(0x82, 0xaa, 0xff);
    pub(super) const CYAN: Color = Color::Rgb(0x86, 0xe1, 0xfc);
    pub(super) const TEAL: Color = Color::Rgb(0x4f, 0xd6, 0xbe);
    pub(super) const GREEN: Color = Color::Rgb(0xc3, 0xe8, 0x8d);
    pub(super) const YELLOW: Color = Color::Rgb(0xff, 0xc7, 0x77);
    pub(super) const ORANGE: Color = Color::Rgb(0xff, 0x96, 0x6c);
    pub(super) const MAGENTA: Color = Color::Rgb(0xc0, 0x99, 0xff);
    pub(super) const PURPLE: Color = Color::Rgb(0xfc, 0xa7, 0xea);
    pub(super) const RED: Color = Color::Rgb(0xff, 0x75, 0x7f);
}

/// The styles of everything muttrs draws.
#[derive(Clone, Debug)]
#[allow(dead_code)] // Until the Index and the sidebar draw them.
pub(crate) struct Theme {
    /// Secondary text: dates, hints, rules, the names of headers.
    pub(crate) dim: Style,
    /// What has focus, or wants attention without alarm.
    pub(crate) accent: Style,
    /// The row under the cursor, in the pane with focus.
    pub(crate) cursor: Style,
    /// The row under the cursor in a pane without focus, and the open Mailbox.
    pub(crate) cursor_unfocused: Style,
    /// Unread Messages, and Mailboxes with Unread Messages.
    pub(crate) unread: Style,
    /// Read Messages.
    pub(crate) read: Style,
    /// Flagged Messages' mark.
    pub(crate) flagged: Style,
    /// A Message's subject in the Pager.
    pub(crate) subject: Style,
    /// Attachments, in the Pager's headers and their list.
    pub(crate) attachment: Style,
    /// The line saying Remote Content is blocked.
    pub(crate) banner: Style,
    /// Text a search found, and the match the search is at.
    pub(crate) found: Style,
    pub(crate) found_current: Style,
    /// The status line.
    pub(crate) status: Style,
    /// What's wrong.
    pub(crate) error: Style,
    /// The keys in hints and help.
    pub(crate) keys: Style,
    /// Bodies.
    pub(crate) body: Styles,
}

impl Theme {
    /// The theme called `name`, drawing `icons`; `None` for no theme of that name.
    pub(crate) fn named(name: &str, icons: &Icons) -> Option<Self> {
        match name {
            "tokyonight-moon" => Some(Self::moon(icons)),
            "terminal" => Some(Self::terminal(icons)),
            _ => None,
        }
    }

    /// The names of the built-in themes.
    pub(crate) const NAMES: [&'static str; 2] = ["tokyonight-moon", "terminal"];

    fn glyphs(icons: &Icons) -> Glyphs {
        Glyphs {
            image: icons.image.to_owned(),
            ..Glyphs::default()
        }
    }

    fn moon(icons: &Icons) -> Self {
        let text = Style::new().fg(moon::FG);
        let dim = Style::new().fg(moon::COMMENT);
        let (r, g, b) = moon::BG;
        Self {
            dim,
            accent: Style::new().fg(moon::BLUE),
            cursor: Style::new().bg(moon::BG_VISUAL),
            cursor_unfocused: Style::new().bg(moon::BG_HIGHLIGHT),
            unread: Style::new().fg(moon::FG).add_modifier(Modifier::BOLD),
            read: Style::new().fg(moon::FG_DARK),
            flagged: Style::new().fg(moon::ORANGE),
            subject: Style::new().fg(moon::FG).add_modifier(Modifier::BOLD),
            attachment: Style::new().fg(moon::TEAL),
            banner: Style::new().fg(moon::YELLOW),
            found: Style::new().fg(Color::Rgb(r, g, b)).bg(moon::YELLOW),
            found_current: Style::new().fg(Color::Rgb(r, g, b)).bg(moon::ORANGE),
            status: Style::new().fg(moon::FG_DARK).bg(moon::BG_HIGHLIGHT),
            error: Style::new().fg(moon::RED),
            keys: Style::new().fg(moon::BLUE).add_modifier(Modifier::BOLD),
            body: Styles {
                text,
                page: Some(Rgb(r, g, b)),
                link: Style::new()
                    .fg(moon::TEAL)
                    .add_modifier(Modifier::UNDERLINED),
                link_number: dim,
                heading: Style::new().fg(moon::BLUE).add_modifier(Modifier::BOLD),
                code: Style::new().fg(moon::GREEN),
                mark: Style::new().fg(Color::Rgb(r, g, b)).bg(moon::YELLOW),
                quotes: vec![
                    Style::new().fg(moon::BLUE),
                    Style::new().fg(moon::MAGENTA),
                    Style::new().fg(moon::CYAN),
                    Style::new().fg(moon::PURPLE),
                ],
                attribution: Style::new()
                    .fg(moon::FG_DARK)
                    .add_modifier(Modifier::ITALIC),
                signature: dim,
                rule: dim,
                table_header: Style::new().add_modifier(Modifier::BOLD),
                image: Style::new()
                    .fg(moon::MAGENTA)
                    .add_modifier(Modifier::ITALIC),
                glyphs: Self::glyphs(icons),
            },
        }
    }

    fn terminal(icons: &Icons) -> Self {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        Self {
            dim,
            accent: bold,
            cursor: Style::new().add_modifier(Modifier::REVERSED),
            cursor_unfocused: Style::new().add_modifier(Modifier::UNDERLINED),
            unread: bold,
            read: Style::new(),
            flagged: Style::new().fg(Color::Yellow),
            subject: bold,
            attachment: Style::new().fg(Color::Cyan),
            banner: Style::new().fg(Color::Yellow),
            found: Style::new().fg(Color::Black).bg(Color::Yellow),
            found_current: Style::new().add_modifier(Modifier::REVERSED),
            status: Style::new().add_modifier(Modifier::REVERSED),
            error: Style::new().fg(Color::Red),
            keys: bold,
            body: Styles {
                glyphs: Self::glyphs(icons),
                ..Styles::default()
            },
        }
    }
}
