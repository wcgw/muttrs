//! The configuration file: `config.toml` in muttrs' configuration directory (see [`crate::xdg`]). Everything in it
//! is optional.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use muttrs_mail::AccountConfig;
use serde::Deserialize;

use crate::xdg;

/// What the configuration file says.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
// Each bool is a setting of its own, not a state machine folded into bools.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Config {
    /// The theme: `tokyonight-moon`, `terminal`, or a theme of the user's in `themes/NAME.toml`.
    pub(crate) theme: String,
    /// Whether to draw Nerd Font icons: `true`, `false`, or `"auto"` for terminals that have the symbols built in.
    pub(crate) icons: Auto,
    /// How to draw Pictures: `"auto"`, `"kitty"`, `"blocks"`, or `"off"`.
    pub(crate) images: Images,
    /// Whether to load Remote Content without being asked. Loading it tells senders their Message was read.
    pub(crate) load_remote_content: bool,
    /// Which body to show of a Message that has both: `"html"` or `"plain"`.
    pub(crate) prefer: Prefer,
    /// The widest the Pager lays bodies out, in cells, however wide the terminal.
    pub(crate) pager_width: u16,
    /// How many rows the Index keeps above the Pager, its status line among them, as mutt's `pager_index_lines`;
    /// 0 for none. At most half the screen.
    pub(crate) pager_index_lines: u16,
    /// How many seconds the Pager shows a Message before it's marked read, as NeoMutt's `pager_read_delay`: stepping
    /// to another, or closing the Pager, before then leaves it Unread. 0 marks it read as it opens.
    pub(crate) pager_read_delay: u64,
    /// The most rows a Picture takes.
    pub(crate) max_picture_rows: u16,
    /// How many of the terminal's pixels make a CSS pixel, which HTML sizes images in; guessed from the size of the
    /// terminal's cells by default (2 on most HiDPI screens).
    pub(crate) pixel_ratio: Option<f32>,
    /// Where Attachments are saved; `~/Downloads` by default.
    pub(crate) download_dir: Option<String>,
    /// The editor Drafts are written in; `$VISUAL`, `$EDITOR` or `vi` by default.
    pub(crate) editor: Option<String>,
    /// Whether the Index shows Messages in threads, at first; `T` switches.
    pub(crate) threads: bool,
    /// Whether a reply in a thread leaves its subject out, as an arrow, where it's what it replies to's (`Re:`
    /// aside), as mutt's `hide_thread_subject` does.
    pub(crate) hide_thread_subjects: bool,
    /// Whether muttrs takes the mouse: its wheel scrolls, its clicks choose. Terminals then select text with Shift.
    pub(crate) mouse: bool,
    /// The Accounts, as `[[account]]` tables, in the sidebar's order.
    #[serde(rename = "account")]
    pub(crate) accounts: Vec<AccountConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "tokyonight-moon".into(),
            icons: Auto::Detect,
            images: Images::Auto,
            load_remote_content: false,
            prefer: Prefer::Html,
            pager_width: 100,
            pager_index_lines: 0,
            pager_read_delay: 1,
            max_picture_rows: 24,
            pixel_ratio: None,
            download_dir: None,
            editor: None,
            threads: true,
            hide_thread_subjects: false,
            mouse: true,
            accounts: Vec::new(),
        }
    }
}

/// `true`, `false`, or `"auto"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Auto {
    /// As suits the terminal.
    Detect,
    On,
    Off,
}

impl<'de> Deserialize<'de> for Auto {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Bool(bool),
            Word(String),
        }
        match Written::deserialize(deserializer)? {
            Written::Bool(true) => Ok(Self::On),
            Written::Bool(false) => Ok(Self::Off),
            Written::Word(word) if word == "auto" => Ok(Self::Detect),
            Written::Word(word) => Err(serde::de::Error::custom(format!(
                "expected true, false or \"auto\", not {word:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Images {
    Auto,
    Kitty,
    Blocks,
    Off,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Prefer {
    Html,
    Plain,
}

impl Config {
    /// The configuration file's path, where there's a configuration directory.
    pub(crate) fn path() -> Option<PathBuf> {
        Some(xdg::config_dir()?.join("config.toml"))
    }

    /// Reads the configuration file; the defaults if there's none.
    ///
    /// # Errors
    ///
    /// If it can't be read, or doesn't say what muttrs understands.
    pub(crate) fn load() -> anyhow::Result<Self> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, &path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn parse(text: &str, path: &Path) -> anyhow::Result<Self> {
        let config: Self =
            toml::from_str(text).with_context(|| format!("in {}", path.display()))?;
        // The theme is checked when it's loaded: it may be a file of the user's.
        if config
            .pixel_ratio
            .is_some_and(|ratio| !(0.25..=8.0).contains(&ratio))
        {
            bail!("in {}: pixel_ratio is between 0.25 and 8", path.display());
        }
        let mut names = std::collections::HashSet::new();
        for account in &config.accounts {
            if !names.insert(account.name.as_str()) {
                bail!(
                    "in {}: two accounts are called {:?}",
                    path.display(),
                    account.name
                );
            }
        }
        if config.pager_width < 20 {
            bail!("in {}: pager_width is at least 20", path.display());
        }
        Ok(config)
    }

    /// Where Attachments are saved.
    pub(crate) fn download_dir(&self) -> PathBuf {
        self.download_dir.as_deref().map_or_else(
            || xdg::home().map_or_else(|| PathBuf::from("."), |home| home.join("Downloads")),
            xdg::expand,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> anyhow::Result<Config> {
        Config::parse(text, Path::new("config.toml"))
    }

    #[test]
    fn everything_is_optional() {
        let config = parse("").unwrap();
        assert_eq!(config.theme, "tokyonight-moon");
        assert_eq!(config.icons, Auto::Detect);
        assert!(!config.load_remote_content);
        assert_eq!((config.pager_index_lines, config.pager_read_delay), (0, 1));
    }

    #[test]
    fn what_it_takes() {
        let config = parse(
            "theme = \"terminal\"\nicons = false\nimages = \"blocks\"\nprefer = \"plain\"\npager_width = 80\n\
             pager_index_lines = 8\npager_read_delay = 0\n",
        )
        .unwrap();
        assert_eq!((config.pager_index_lines, config.pager_read_delay), (8, 0));
        assert_eq!(config.icons, Auto::Off);
        assert_eq!(config.images, Images::Blocks);
        assert_eq!(config.prefer, Prefer::Plain);
        assert_eq!(config.pager_width, 80);
    }

    #[test]
    fn accounts() {
        let config = parse(
            "[[account]]\nname = \"work\"\nemail = \"a@example.com\"\nprovider = \"gmail\"\n\
             [account.oauth2]\nclient_id = \"id\"\n\n\
             [[account]]\nname = \"home\"\nemail = \"b@example.com\"\nimap = { host = \"imap.example.com\" }\n\
             smtp = { host = \"smtp.example.com\", port = 587, tls = \"starttls\" }\n",
        )
        .unwrap();
        let names: Vec<&str> = config
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect();
        assert_eq!(names, ["work", "home"]);
        assert!(parse("[[account]]\nname = \"a\"\nemail = \"a@x\"\n[[account]]\nname = \"a\"\nemail = \"b@x\"").is_err());
    }

    #[test]
    fn mistakes_are_errors() {
        assert!(parse("icons = \"sometimes\"").is_err());
        assert!(parse("colour = 1").is_err(), "unknown keys aren't ignored");
    }
}
