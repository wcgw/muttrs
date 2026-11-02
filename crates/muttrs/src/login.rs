//! `muttrs login NAME`: signs in to an Account once, keeping what it takes to sign in again in the keyring, then
//! checks it works by signing in to its IMAP server.

use std::io::{self, Write as _};

use anyhow::{Context as _, anyhow, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use muttrs_mail::account::{Auth, PasswordSource};
use muttrs_mail::imap::{Credentials, Session};
use muttrs_mail::secret::{self, Kind};
use muttrs_mail::{client, oauth2, tls};

use crate::config::Config;

pub(crate) async fn login(name: &str) -> anyhow::Result<()> {
    let config = Config::load()?;
    let names: Vec<&str> = config
        .accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect();
    let account = config
        .accounts
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
        .resolve()?;
    let credentials = match &account.auth {
        Auth::OAuth2(settings) => {
            let client_secret = client::client_secret(settings.client_secret.as_ref()).await?;
            println!("Signing in to {name} as {}, in the browser…", account.email);
            let tokens =
                oauth2::authorize(settings, client_secret.as_deref(), &account.email, |url| {
                    println!("If no browser opens, open this:\n\n  {url}\n");
                    let _ = open::that_detached(url);
                })
                .await?;
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
    let tls = tls::client_config(account.ca_file.as_deref())?;
    let session = Session::connect(&account.imap, tls, &credentials)
        .await
        .context("signing in failed")?;
    session.logout().await;
    println!("done.");
    Ok(())
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
