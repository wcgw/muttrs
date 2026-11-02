//! muttrs' colours. Two are built in: Tokyo Night Moon (folke/tokyonight.nvim's `moon`), as ghd and slackers have,
//! and the terminal's own colours with emphasis only. Only text and highlights are coloured: everywhere else the
//! terminal's background shows.
//!
//! A theme of the user's is a file, `themes/NAME.toml` in the configuration directory, that restyles any of a built-in
//! theme's styles (its `base`); `themes/` in muttrs' repository has one to start from.

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

    /// The theme called `name`: a built-in one, or the user's, `themes/NAME.toml` in `config_dir`.
    ///
    /// # Errors
    ///
    /// If there's no theme of that name, or its file doesn't say what muttrs understands.
    pub(crate) fn load(
        name: &str,
        icons: &Icons,
        config_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        if let Some(theme) = Self::named(name, icons) {
            return Ok(theme);
        }
        let path =
            config_dir.map(|directory| directory.join("themes").join(format!("{name}.toml")));
        let Some(text) = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            anyhow::bail!(
                "there's no theme {name:?}: the built-in ones are {}, and a theme of your own goes in {}",
                Self::NAMES.join(", "),
                path.map_or_else(
                    || "themes/NAME.toml".into(),
                    |path| path.display().to_string()
                )
            );
        };
        let path = path.expect("a path, as the file was read");
        let file: file::ThemeFile =
            toml::from_str(&text).with_context(|| format!("in {}", path.display()))?;
        file.apply(icons)
            .with_context(|| format!("in {}", path.display()))
    }

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

/// Themes in files: styles by name, over a built-in theme's.
mod file {
    use std::str::FromStr as _;

    use anyhow::{Context as _, anyhow, bail};
    use ratatui::style::{Color, Modifier, Style};
    use serde::Deserialize;

    use super::Theme;
    use crate::icons::Icons;

    /// A theme as its file writes it.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct ThemeFile {
        /// The built-in theme it restyles; Tokyo Night Moon by default.
        base: Option<String>,
        /// The background authors' colours are judged against: the terminal's, as `#rrggbb`; `"none"` draws
        /// bodies in the theme's colours alone.
        page: Option<String>,
        #[serde(default)]
        ui: Ui,
        #[serde(default)]
        body: Body,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ui {
        dim: Option<Spec>,
        accent: Option<Spec>,
        cursor: Option<Spec>,
        cursor_unfocused: Option<Spec>,
        unread: Option<Spec>,
        read: Option<Spec>,
        flagged: Option<Spec>,
        subject: Option<Spec>,
        attachment: Option<Spec>,
        banner: Option<Spec>,
        found: Option<Spec>,
        found_current: Option<Spec>,
        status: Option<Spec>,
        error: Option<Spec>,
        keys: Option<Spec>,
    }

    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Body {
        text: Option<Spec>,
        link: Option<Spec>,
        link_number: Option<Spec>,
        heading: Option<Spec>,
        code: Option<Spec>,
        mark: Option<Spec>,
        /// A quote's bar, by depth.
        quotes: Option<Vec<Spec>>,
        attribution: Option<Spec>,
        signature: Option<Spec>,
        rule: Option<Spec>,
        table_header: Option<Spec>,
        image: Option<Spec>,
    }

    /// A style: colours, by name (`red`, `lightblue`), as `#rrggbb` or by index (`"208"`), and emphasis.
    #[derive(Debug, Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(clippy::struct_excessive_bools)] // One a modifier, each on or off.
    struct Spec {
        fg: Option<String>,
        bg: Option<String>,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        dim: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        underlined: bool,
        #[serde(default)]
        reversed: bool,
        #[serde(default)]
        crossed_out: bool,
    }

    fn color(text: &str) -> anyhow::Result<Color> {
        Color::from_str(text)
            .map_err(|_| anyhow!("{text:?} isn't a colour: a name, #rrggbb, or 0 to 255"))
    }

    impl Spec {
        fn style(&self) -> anyhow::Result<Style> {
            let mut style = Style::new();
            if let Some(fg) = &self.fg {
                style = style.fg(color(fg)?);
            }
            if let Some(bg) = &self.bg {
                style = style.bg(color(bg)?);
            }
            for (on, modifier) in [
                (self.bold, Modifier::BOLD),
                (self.dim, Modifier::DIM),
                (self.italic, Modifier::ITALIC),
                (self.underlined, Modifier::UNDERLINED),
                (self.reversed, Modifier::REVERSED),
                (self.crossed_out, Modifier::CROSSED_OUT),
            ] {
                if on {
                    style = style.add_modifier(modifier);
                }
            }
            Ok(style)
        }
    }

    /// Sets `style` to what `spec` says, if it says anything; `name` says which, should it be wrong.
    fn set(style: &mut Style, spec: Option<&Spec>, name: &str) -> anyhow::Result<()> {
        if let Some(spec) = spec {
            *style = spec.style().with_context(|| name.to_owned())?;
        }
        Ok(())
    }

    impl ThemeFile {
        /// The theme: the base's, restyled as the file says.
        pub(super) fn apply(self, icons: &Icons) -> anyhow::Result<Theme> {
            let base = self.base.as_deref().unwrap_or("tokyonight-moon");
            let Some(mut theme) = Theme::named(base, icons) else {
                bail!(
                    "base = {base:?}: a theme's base is a built-in one, {}",
                    Theme::NAMES.join(" or ")
                );
            };
            let ui = &self.ui;
            for (style, spec, name) in [
                (&mut theme.dim, &ui.dim, "ui.dim"),
                (&mut theme.accent, &ui.accent, "ui.accent"),
                (&mut theme.cursor, &ui.cursor, "ui.cursor"),
                (
                    &mut theme.cursor_unfocused,
                    &ui.cursor_unfocused,
                    "ui.cursor_unfocused",
                ),
                (&mut theme.unread, &ui.unread, "ui.unread"),
                (&mut theme.read, &ui.read, "ui.read"),
                (&mut theme.flagged, &ui.flagged, "ui.flagged"),
                (&mut theme.subject, &ui.subject, "ui.subject"),
                (&mut theme.attachment, &ui.attachment, "ui.attachment"),
                (&mut theme.banner, &ui.banner, "ui.banner"),
                (&mut theme.found, &ui.found, "ui.found"),
                (
                    &mut theme.found_current,
                    &ui.found_current,
                    "ui.found_current",
                ),
                (&mut theme.status, &ui.status, "ui.status"),
                (&mut theme.error, &ui.error, "ui.error"),
                (&mut theme.keys, &ui.keys, "ui.keys"),
            ] {
                set(style, spec.as_ref(), name)?;
            }
            let body = &self.body;
            let styles = &mut theme.body;
            for (style, spec, name) in [
                (&mut styles.text, &body.text, "body.text"),
                (&mut styles.link, &body.link, "body.link"),
                (
                    &mut styles.link_number,
                    &body.link_number,
                    "body.link_number",
                ),
                (&mut styles.heading, &body.heading, "body.heading"),
                (&mut styles.code, &body.code, "body.code"),
                (&mut styles.mark, &body.mark, "body.mark"),
                (
                    &mut styles.attribution,
                    &body.attribution,
                    "body.attribution",
                ),
                (&mut styles.signature, &body.signature, "body.signature"),
                (&mut styles.rule, &body.rule, "body.rule"),
                (
                    &mut styles.table_header,
                    &body.table_header,
                    "body.table_header",
                ),
                (&mut styles.image, &body.image, "body.image"),
            ] {
                set(style, spec.as_ref(), name)?;
            }
            if let Some(quotes) = &body.quotes {
                if quotes.is_empty() {
                    bail!("body.quotes: at least one style");
                }
                styles.quotes = quotes
                    .iter()
                    .map(Spec::style)
                    .collect::<anyhow::Result<_>>()
                    .context("body.quotes")?;
            }
            match self.page.as_deref() {
                None => {}
                Some("none") => styles.page = None,
                Some(page) => match color(page).context("page")? {
                    Color::Rgb(r, g, b) => styles.page = Some(muttrs_render::Rgb(r, g, b)),
                    _ => bail!("page: a colour as #rrggbb, to judge authors' colours against"),
                },
            }
            Ok(theme)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn theme(text: &str) -> anyhow::Result<Theme> {
            toml::from_str::<ThemeFile>(text)?.apply(&Icons::PLAIN)
        }

        #[test]
        fn a_file_restyles_its_base() {
            let theme = theme(
                "base = \"terminal\"\npage = \"#1e1e2e\"\n[ui]\naccent = { fg = \"#89b4fa\", bold = true }\n\
                 [body]\nquotes = [{ fg = \"blue\" }, { fg = \"208\" }]",
            )
            .unwrap();
            assert_eq!(
                theme.accent,
                Style::new()
                    .fg(Color::Rgb(0x89, 0xb4, 0xfa))
                    .add_modifier(Modifier::BOLD)
            );
            assert_eq!(theme.body.quotes[1], Style::new().fg(Color::Indexed(208)));
            assert_eq!(theme.body.page, Some(muttrs_render::Rgb(0x1e, 0x1e, 0x2e)));
            assert_eq!(
                theme.cursor,
                Theme::named("terminal", &Icons::PLAIN).unwrap().cursor,
                "the base's"
            );
        }

        #[test]
        fn the_shipped_theme_loads() {
            let shipped = include_str!("../../../themes/catppuccin-mocha.toml");
            let theme = theme(shipped).unwrap();
            assert_eq!(theme.body.page, Some(muttrs_render::Rgb(0x1e, 0x1e, 0x2e)));
            assert_eq!(theme.body.quotes.len(), 4);
        }

        #[test]
        fn mistakes_say_where() {
            let error = theme("[ui]\naccent = { fg = \"chartreuse-ish\" }").unwrap_err();
            assert!(format!("{error:#}").contains("ui.accent"), "{error:#}");
            assert!(
                theme("[ui]\naccents = {}").is_err(),
                "unknown keys aren't ignored"
            );
            assert!(theme("base = \"mine\"").is_err());
        }
    }
}
