//! The configuration file: `config.toml` in muttrs' configuration directory (see [`crate::xdg`]). Everything in it
//! is optional.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use muttrs_mail::AccountConfig;
use serde::Deserialize;

use crate::assessment::Device;
use crate::xdg;

/// What the configuration file says.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
// Each bool is a setting of its own, not a state machine folded into bools.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Config {
    /// Read no more, as ghd's: `:colorscheme` in `init.vim` says how muttrs looks.
    pub(crate) theme: Option<toml::Value>,
    /// Read no more: `:set icons=` in `init.vim` says how icons are drawn.
    pub(crate) icons: Option<toml::Value>,
    /// Read no more: `:set graphics=` in `init.vim` says how Pictures are drawn.
    pub(crate) images: Option<toml::Value>,
    /// Whether to load Remote Content without being asked. Loading it tells senders their Message was read.
    pub(crate) load_remote_content: bool,
    /// Which body to show of a Message that has both: `"html"` or `"plain"`.
    pub(crate) prefer: Prefer,
    /// Read no more: `:set textwidth=` in `init.vim` says the widest prose is laid out, in every app.
    pub(crate) pager_width: Option<toml::Value>,
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
    /// The editor Drafts are written in; `$EDITOR`, else `$VISUAL`, by default.
    pub(crate) editor: Option<String>,
    /// Whether the Index shows Messages in threads, at first; `T` switches.
    pub(crate) threads: bool,
    /// Whether a reply in a thread leaves its subject out, as an arrow, where it's what it replies to's (`Re:`
    /// aside), as mutt's `hide_thread_subject` does.
    pub(crate) hide_thread_subjects: bool,
    /// Read no more: `:set mouse=` in `init.vim` says whether muttrs takes the mouse.
    pub(crate) mouse: Option<toml::Value>,
    /// Where voight assesses the Pager's prose: `"cpu"` (the default, if muttrs was built with voight), `"metal"`,
    /// `"cuda"` or `"wgpu"` in a muttrs built for them, or `"off"`.
    pub(crate) voight: Voight,
    /// A command that finds addresses to complete with, as mutt's `query_command` (`khard email --parsable %s`,
    /// `abook --mutt-query %s`): `%s` is what's typed.
    pub(crate) query_command: Option<String>,
    /// Keys of one's own, as muttrs had them: no longer read, as `:map` in `init.vim` maps keys now
    /// (`scripts/convert-config.py` writes them so).
    pub(crate) keys: Option<toml::Table>,
    /// The Accounts, as `[[account]]` tables, in the sidebar's order.
    #[serde(rename = "account")]
    pub(crate) accounts: Vec<AccountConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: None,
            icons: None,
            images: None,
            load_remote_content: false,
            prefer: Prefer::Html,
            pager_width: None,
            pager_index_lines: 0,
            pager_read_delay: 1,
            max_picture_rows: 24,
            pixel_ratio: None,
            download_dir: None,
            editor: None,
            threads: true,
            hide_thread_subjects: false,
            mouse: None,
            voight: Voight::default(),
            query_command: None,
            keys: None,
            accounts: Vec::new(),
        }
    }
}

/// The `voight` setting: `"off"`, or the device to run on.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Voight {
    Off,
    Cpu,
    Metal,
    Cuda,
    Wgpu,
}

impl Default for Voight {
    /// On the CPU, if muttrs was built with voight.
    fn default() -> Self {
        if cfg!(feature = "voight") {
            Self::Cpu
        } else {
            Self::Off
        }
    }
}

impl Voight {
    /// The device voight runs on; `None` when it's off.
    pub(crate) fn device(self) -> Option<Device> {
        match self {
            Self::Off => None,
            Self::Cpu => Some(Device::Cpu),
            Self::Metal => Some(Device::Metal),
            Self::Cuda => Some(Device::Cuda),
            Self::Wgpu => Some(Device::Wgpu),
        }
    }
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
        match config.voight.device() {
            Some(device) if !cfg!(feature = "voight") => bail!(
                "in {}: voight = \"{device}\", but this muttrs was built without voight: set voight = \"off\", \
                 or build muttrs with its `voight` feature",
                path.display()
            ),
            Some(device) if !device.is_built_in() => bail!(
                "in {}: voight = \"{device}\", but this muttrs was built without {device} support: set voight = \
                 \"cpu\", or build muttrs with `--features {device}`",
                path.display()
            ),
            _ => {}
        }
        Ok(config)
    }

    /// What's said of the settings it has that are read no more: the look's, which `init.vim` says, as in ghd, and
    /// `[keys.*]`, which `:map` says.
    pub(crate) fn ignored(&self) -> Vec<String> {
        let mut ignored = Vec::new();
        let look: Vec<&str> = [
            ("theme", self.theme.is_some()),
            ("icons", self.icons.is_some()),
            ("images", self.images.is_some()),
            ("mouse", self.mouse.is_some()),
        ]
        .into_iter()
        .filter_map(|(key, set)| set.then_some(key))
        .collect();
        if let Some((last, rest)) = look.split_last() {
            let keys = if rest.is_empty() {
                (*last).to_owned()
            } else {
                format!("{} and {last}", rest.join(", "))
            };
            ignored.push(format!(
                "{keys} read no more: :colorscheme and :set icons= graphics= mouse= go in \
                 ~/.config/muttrs/init.vim (scripts/convert-config.py writes them)"
            ));
        }
        if self.pager_width.is_some() {
            ignored.push(
                "pager_width read no more: :set textwidth= in ~/.config/muttrs/init.vim says the widest prose is laid \
                 out (scripts/convert-config.py writes it)"
                    .to_owned(),
            );
        }
        if self.keys.is_some() {
            ignored.push(
                "[keys.*] are no longer read: :map keys in ~/.config/muttrs/init.vim (scripts/convert-config.py)"
                    .to_owned(),
            );
        }
        ignored
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
        assert_eq!(config.ignored(), Vec::<String>::new());
        assert!(!config.load_remote_content);
        assert_eq!((config.pager_index_lines, config.pager_read_delay), (0, 1));
        assert_eq!(
            config.voight.device(),
            cfg!(feature = "voight").then_some(Device::Cpu)
        );
    }

    #[test]
    fn what_it_takes() {
        let config = parse(
            "theme = \"terminal\"\nicons = false\nimages = \"blocks\"\nprefer = \"plain\"\npager_width = 80\n\
             pager_index_lines = 8\npager_read_delay = 0\n",
        )
        .unwrap();
        assert_eq!((config.pager_index_lines, config.pager_read_delay), (8, 0));
        assert_eq!(config.prefer, Prefer::Plain);
        assert!(config.ignored()[1].starts_with("pager_width read no more"));
    }

    #[test]
    fn voight_runs_where_muttrs_was_built_to() {
        assert_eq!(parse("voight = \"off\"\n").unwrap().voight, Voight::Off);
        let cuda = parse("voight = \"cuda\"\n");
        let refused = |cuda: anyhow::Result<Config>, why: &str| {
            assert!(cuda.unwrap_err().to_string().contains(why));
        };
        if cfg!(feature = "cuda") {
            assert_eq!(cuda.unwrap().voight, Voight::Cuda);
        } else if cfg!(feature = "voight") {
            refused(cuda, "without cuda support");
        } else {
            refused(cuda, "without voight");
        }
        assert!(parse("voight = \"tpu\"\n").is_err());
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
    fn the_look_is_read_no_more_but_still_loads_as_in_ghd() {
        let config = parse(
            "theme = \"terminal\"\nicons = \"sometimes\"\nimages = \"blocks\"\nmouse = false\neditor = \"nvim\"\n\
             [keys.index]\nx = \"a\"\n",
        )
        .unwrap();
        let ignored = config.ignored();
        assert_eq!(ignored.len(), 2, "the look's, and [keys.*]: {ignored:?}");
        assert!(
            ignored[0].starts_with("theme, icons, images and mouse read no more"),
            "{ignored:?}"
        );
        assert!(parse("mouse = false").unwrap().ignored()[0].starts_with("mouse read no more"));
        assert_eq!(
            config.editor.as_deref(),
            Some("nvim"),
            "the editor is muttrs' own again"
        );
        assert_eq!(parse("").unwrap().ignored(), Vec::<String>::new());
    }

    #[test]
    fn mistakes_are_errors() {
        assert!(parse("colour = 1").is_err(), "unknown keys aren't ignored");
    }
}
