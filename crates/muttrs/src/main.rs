//! muttrs: a terminal mail client after mutt and vim, that renders HTML mail, images included.

mod addresses;
mod app;
mod assessment;
mod compose;
mod config;
mod filter;
mod heatmap;
mod hl;
mod icons;
mod index;
mod login;
mod net;
mod pager;
mod remote;
mod sidebar;
mod sign_in;
mod tasks;
mod trust;
mod xdg;

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context as _, bail};
use muttrs_mail::Message;
use muttrs_mail::client::{Client, Connection};
use wcgw_kit::runtime::{self, Inbox};

use crate::addresses::AddressBook;
use crate::app::{Msg, Muttrs};
use crate::config::Config;
use crate::net::Net;
use crate::sidebar::AccountState;

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

/// voight, started on the device the configuration says, if it's on, telling `inbox` what it says: before the
/// terminal UI takes over the screen, as the first time, it downloads its model, showing its progress.
#[cfg(feature = "voight")]
async fn voight(
    config: &Config,
    inbox: &Inbox<Msg>,
) -> anyhow::Result<Option<assessment::Assessor>> {
    let Some(device) = config.voight.device() else {
        return Ok(None);
    };
    let inbox = inbox.clone();
    let started = tokio::task::spawn_blocking(move || {
        assessment::start(device, move |said| {
            let _ = inbox.send(Msg::Voight(said));
        })
    })
    .await
    .context("fetching voight's model stopped")?;
    Ok(Some(started.context(
        "couldn't fetch voight's model; set voight = \"off\" in the configuration to start without it",
    )?))
}

/// What carries out muttrs' effects, with voight if it's on; and whether it is.
#[cfg_attr(not(feature = "voight"), allow(clippy::unused_async))]
async fn net(
    config: &Config,
    inbox: &Inbox<Msg>,
    pictures: &pager::Pictures,
) -> anyhow::Result<(Net, bool)> {
    let net = Net::new(inbox.clone(), config.accounts.clone(), Arc::clone(pictures));
    #[cfg(feature = "voight")]
    {
        let assessor = voight(config, inbox).await?;
        let on = assessor.is_some();
        Ok((net.with_assessor(assessor), on))
    }
    #[cfg(not(feature = "voight"))]
    {
        Ok((net, false))
    }
}

async fn mail() -> anyhow::Result<()> {
    let config = Config::load()?;
    if config.accounts.is_empty() {
        bail!(
            "there are no accounts in {}: add an [[account]] (see the README), or `muttrs view FILE` reads a file",
            Config::path().map_or_else(|| "config.toml".into(), |path| path.display().to_string())
        );
    }
    let accounts = config
        .accounts
        .iter()
        .cloned()
        .map(muttrs_mail::AccountConfig::resolve)
        .collect::<Result<Vec<_>, _>>()?;
    let (inbox, mailbox) = runtime::channel();
    let pictures = pager::Pictures::default();
    let (net, voight) = net(&config, &inbox, &pictures).await?;
    let mut states = Vec::new();
    for (index, account) in accounts.into_iter().enumerate() {
        let name = account.name.clone();
        // The address book's contacts, aside, once at the start.
        if let Some(book) = account.carddav.clone() {
            let (inbox, name) = (inbox.clone(), name.clone());
            tokio::spawn(async move {
                let contacts = muttrs_mail::carddav::contacts(&book, &name)
                    .await
                    .map_err(|error| error.to_string());
                let _ = inbox.send(Msg::Contacts(name, contacts));
            });
        }
        let sender_of_drafts = account.sender();
        let cache = muttrs_mail::cache::Cache::new(
            xdg::cache_dir().map(|cache| cache.join("accounts").join(&name)),
        );
        let told = inbox.clone();
        let client = Client::spawn(
            account,
            cache.clone(),
            Arc::new(move |update| {
                let _ = told.send(Msg::Account(index, update));
            }),
        )
        .with_context(|| format!("setting up account {name:?}"))?;
        states.push(AccountState {
            name,
            client,
            connection: Connection::Connecting,
            // As last listed, until the server lists them.
            mailboxes: cache.mailboxes().unwrap_or_default(),
            labels: Vec::new(),
            counts: std::collections::HashMap::new(),
            sender: sender_of_drafts,
            cache,
            tasks: crate::tasks::Tasks::default(),
        });
    }
    let addresses = AddressBook::load(xdg::cache_dir().map(|cache| cache.join("addresses.txt")));
    let (muttrs, kit) = Muttrs::mail(config, states, addresses, voight, pictures);
    tracing::info!("starting");
    let (mut muttrs, _) = runtime::run_returning("muttrs", muttrs, kit, net, (inbox, mailbox))
        .await
        .context("running the terminal")?;
    muttrs.save();
    Ok(())
}

async fn view(path: &str) -> anyhow::Result<()> {
    let config = Config::load()?;
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading {path}"))?;
    if bytes.is_empty() {
        bail!("{path} is empty");
    }
    let message = Message::parse(bytes).with_context(|| format!("reading {path}"))?;
    let (inbox, mailbox) = runtime::channel();
    let pictures = pager::Pictures::default();
    let (net, voight) = net(&config, &inbox, &pictures).await?;
    let (muttrs, kit) = Muttrs::view(config, message, voight, pictures);
    runtime::run("muttrs", muttrs, kit, net, (inbox, mailbox))
        .await
        .context("running the terminal")
}
