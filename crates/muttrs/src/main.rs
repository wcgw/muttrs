//! muttrs: a terminal mail client after mutt and vim, that renders HTML mail, images included.

mod config;
mod icons;
mod index;
mod login;
mod overlay;
mod pager;
mod pictures;
mod remote;
mod sidebar;
mod theme;
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
    let theme = Theme::named(&config.theme, &icons).expect("a theme the configuration checked");
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
    })
}

async fn mail() -> anyhow::Result<()> {
    let setup = setup()?;
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
    ui::mail(setup, accounts).await
}

async fn view(path: &str) -> anyhow::Result<()> {
    let setup = setup()?;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading {path}"))?;
    if bytes.is_empty() {
        bail!("{path} is empty");
    }
    let message = Message::parse(bytes).with_context(|| format!("reading {path}"))?;
    ui::view(setup, message).await
}
