//! muttrs: a terminal mail client after mutt and vim, that renders HTML mail, images included.

mod addresses;
mod assessment;
mod compose;
mod config;
mod filter;
mod heatmap;
mod history;
mod icons;
mod index;
mod keys;
mod login;
mod overlay;
mod pager;
mod pictures;
mod remote;
mod sidebar;
mod sign_in;
mod tasks;
mod theme;
mod trust;
mod ui;
mod xdg;

use std::process::ExitCode;

use anyhow::{Context as _, bail};
use muttrs_gfx::{Colors, Protocol, Terminal};
use muttrs_mail::Message;

use crate::config::{Auto, Config, Images};
use crate::icons::Icons;
use crate::theme::Theme;

const USAGE: &str = "\
muttrs, a terminal mail client

Usage:
  muttrs               read and write mail, with the accounts in the configuration
  muttrs login NAME    sign in to account NAME, keeping its password or OAuth2 token in the keyring
                       (`:login NAME`, in muttrs, signs in again with OAuth2 without leaving it)
  muttrs trust NAME    trust the certificates account NAME's servers present, as they are (self-signed ones)
  muttrs view FILE     show a Message saved as a file (.eml), in the Pager
  muttrs --help        show this
  muttrs --version     show the version

The configuration is read from $XDG_CONFIG_HOME/muttrs/config.toml (~/.config/muttrs/config.toml).";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--help" | "-h" | "help"] => {
            println!("{USAGE}");
            Ok(())
        }
        ["--version" | "-V"] => {
            println!("muttrs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [] => run(mail()),
        ["login", name] => run(login::login(name)),
        ["trust", name] => run(trust::trust(name)),
        ["view", path] => run(view(path)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("muttrs: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Runs `task` on a runtime of its own, with logging and TLS set up.
fn run(task: impl Future<Output = anyhow::Result<()>>) -> anyhow::Result<()> {
    log();
    // ring for every TLS connection: reqwest's (Remote Content, OAuth2) takes the process' provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the runtime")?
        .block_on(task)
}

/// Logs to `muttrs.log` in the state directory, at the level `MUTTRS_LOG` says (`info` by default). Without a
/// state directory, nothing's logged: the terminal belongs to the UI.
fn log() {
    let Some(directory) = xdg::state_dir() else {
        return;
    };
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("muttrs.log"))
    else {
        return;
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("MUTTRS_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
}

/// The UI's setup, from the configuration and the terminal.
fn setup() -> anyhow::Result<ui::Setup> {
    let config = Config::load()?;
    let terminal = Terminal::detect();
    let nerd = match config.icons {
        Auto::On => true,
        Auto::Off => false,
        Auto::Detect => terminal.has_nerd_font_symbols,
    };
    let icons = if nerd { Icons::NERD } else { Icons::PLAIN };
    let theme = Theme::load(&config.theme, &icons, xdg::config_dir().as_deref())?;
    let protocol = match config.images {
        Images::Auto => Some(terminal.protocol),
        Images::Kitty => Some(Protocol::Kitty),
        Images::Blocks => Some(match terminal.protocol {
            Protocol::HalfBlocks(colors) => Protocol::HalfBlocks(colors),
            Protocol::Kitty => Protocol::HalfBlocks(Colors::TrueColor),
        }),
        Images::Off => None,
    };
    tracing::info!(?terminal, ?protocol, "starting");
    Ok(ui::Setup {
        config,
        theme,
        icons,
        protocol,
        assessor: None,
    })
}

/// voight, started on the device the configuration says, if it's on: before the terminal UI takes over the screen,
/// as the first time, it downloads its model, showing its progress.
// Built without voight, there's nothing to wait for.
#[cfg_attr(not(feature = "voight"), allow(clippy::unused_async))]
async fn voight(config: &Config) -> anyhow::Result<Option<assessment::Assessor>> {
    match config.voight.device() {
        #[cfg(feature = "voight")]
        Some(device) => Ok(Some(assessment::Assessor::start(device).await.context(
            "couldn't fetch voight's model; set voight = \"off\" in the configuration to start without it",
        )?)),
        #[cfg(not(feature = "voight"))]
        Some(_) => unreachable!("the configuration can't turn voight on in a muttrs built without it"),
        None => Ok(None),
    }
}

async fn mail() -> anyhow::Result<()> {
    let mut setup = setup()?;
    if setup.config.accounts.is_empty() {
        bail!(
            "there are no accounts in {}: add an [[account]] (see the README), or `muttrs view FILE` reads a file",
            Config::path().map_or_else(|| "config.toml".into(), |path| path.display().to_string())
        );
    }
    let accounts = setup
        .config
        .accounts
        .iter()
        .cloned()
        .map(muttrs_mail::AccountConfig::resolve)
        .collect::<Result<Vec<_>, _>>()?;
    setup.assessor = voight(&setup.config).await?;
    ui::mail(setup, accounts).await
}

async fn view(path: &str) -> anyhow::Result<()> {
    let mut setup = setup()?;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading {path}"))?;
    if bytes.is_empty() {
        bail!("{path} is empty");
    }
    let message = Message::parse(bytes).with_context(|| format!("reading {path}"))?;
    setup.assessor = voight(&setup.config).await?;
    ui::view(setup, message).await
}
