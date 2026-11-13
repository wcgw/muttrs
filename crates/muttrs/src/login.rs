//! `muttrs login NAME`: signs in to an Account once, keeping what it takes to sign in again in the keyring, then
//! checks it works by signing in to its IMAP server. An Account that signs in with OAuth2 can sign in again from
//! the UI too, with `:login` (see [`crate::sign_in`]).

use std::io::{self, Write as _};

use anyhow::{Context as _, anyhow, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use muttrs_mail::account::{Auth, PasswordSource};
use muttrs_mail::imap::{Credentials, Session};
use muttrs_mail::secret::{self, Kind};
use muttrs_mail::{Account, client, oauth2, tls};
use tokio::sync::mpsc;

use crate::config::Config;

pub(crate) async fn login(name: &str) -> anyhow::Result<()> {
    let config = Config::load()?;
    let account = account(&config, name)?;
    let credentials = match &account.auth {
        Auth::OAuth2(settings) => {
            let client_secret = client::client_secret(settings.client_secret.as_ref()).await?;
            let authorization = oauth2::Authorization::start(settings, &account.email)?;
            println!(
                "Signing in to {name} as {}, in the browser… If none opens, open this:\n\n  {}\n\n\
                 A browser on another machine can't come back here once you've signed in: paste the address it \
                 ends at instead.",
                account.email,
                authorization.url()
            );
            let _ = open::that_detached(authorization.url());
            let mut pasted = lines_typed();
            let tokens = loop {
                match authorization
                    .tokens(client_secret.as_deref(), &mut pasted)
                    .await
                {
                    Err(error @ oauth2::Error::NotTheRedirect) => println!("{error}"),
                    tokens => break tokens?,
                }
            };
            let refresh_token = tokens
                .refresh_token
                .as_deref()
                .expect("authorize gives a refresh token");
            secret::store(&account.name, Kind::RefreshToken, refresh_token).await?;
            println!("The refresh token is in the keyring.");
            Credentials::Bearer {
                username: account.username.clone(),
                token: tokens.access_token,
            }
        }
        Auth::Password(PasswordSource::Keyring) => {
            let password = read_hidden(&format!("Password for {name} ({}): ", account.username))?;
            if password.is_empty() {
                bail!("no password given: nothing kept");
            }
            secret::store(&account.name, Kind::Password, &password).await?;
            println!("The password is in the keyring.");
            Credentials::Password {
                username: account.username.clone(),
                password,
            }
        }
        Auth::Password(PasswordSource::Command(command)) => {
            println!("{name} reads its password from `{command}`: there's nothing to keep.");
            Credentials::Password {
                username: account.username.clone(),
                password: secret::from_command(command).await?,
            }
        }
    };
    print!("Signing in to {}… ", account.imap);
    io::stdout().flush()?;
    let tls = tls::client_config(&account.trust)?;
    let session = Session::connect(&account.imap, tls, &credentials)
        .await
        .context("signing in failed")?;
    session.logout().await;
    println!("done.");
    Ok(())
}

/// The Account called `name` in `config`, resolved.
pub(crate) fn account(config: &Config, name: &str) -> anyhow::Result<Account> {
    resolve(&config.accounts, name)
}

/// The Account called `name` of `accounts`, resolved.
pub(crate) fn resolve(
    accounts: &[muttrs_mail::AccountConfig],
    name: &str,
) -> anyhow::Result<Account> {
    let names: Vec<&str> = accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect();
    Ok(accounts
        .iter()
        .find(|account| account.name == name)
        .ok_or_else(|| match names.as_slice() {
            [] => anyhow!("there are no accounts in the configuration"),
            names => anyhow!(
                "there's no account {name:?}; there are: {}",
                names.join(", ")
            ),
        })?
        .clone()
        .resolve()?)
}

/// The lines typed at the terminal, as they're typed, until its input closes.
fn lines_typed() -> mpsc::UnboundedReceiver<String> {
    let (typed, lines) = mpsc::unbounded_channel();
    // A thread of its own rather than tokio's stdin, whose blocked read can't be cancelled: the runtime would wait for
    // a line to end. This one ends with the process.
    std::thread::spawn(move || {
        for line in io::stdin().lines() {
            let Ok(line) = line else { break };
            if typed.send(line).is_err() {
                break;
            }
        }
    });
    lines
}

/// A line typed at the terminal without showing it, as a password.
fn read_hidden(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    crossterm::terminal::enable_raw_mode().context("reading the password")?;
    let typed = (|| {
        let mut typed = String::new();
        loop {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Enter => return Ok(typed),
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Esc => return Err(anyhow!("cancelled")),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(anyhow!("cancelled"));
                }
                KeyCode::Char(character) => typed.push(character),
                _ => {}
            }
        }
    })();
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    typed
}
