//! `muttrs trust NAME`: shows the certificates an Account's servers present, and keeps them, once the user says so,
//! to trust as they are: for servers with certificates of their own, self-signed, as mail bridges have.

use std::io::{self, BufRead as _, Write as _};

use anyhow::{Context as _, bail};
use muttrs_mail::account::Tls;
use muttrs_mail::tls::{self, Trust};
use muttrs_mail::{imap, smtp};
use rustls::pki_types::CertificateDer;

use crate::config::Config;
use crate::login::account;
use crate::xdg;

pub(crate) async fn trust(name: &str) -> anyhow::Result<()> {
    let config = Config::load()?;
    let account = account(&config, name)?;
    let mut presented: Vec<CertificateDer<'static>> = Vec::new();
    for (protocol, server) in [("IMAP", &account.imap), ("SMTP", &account.smtp)] {
        if server.tls == Tls::None {
            println!("{protocol} {server}: no TLS, so no certificate");
            continue;
        }
        print!("{protocol} {server}: ");
        io::stdout().flush()?;
        let chain: Vec<Vec<u8>> = if protocol == "IMAP" {
            imap::presented_certificates(server)
                .await?
                .into_iter()
                .map(|certificate| certificate.to_vec())
                .collect()
        } else {
            smtp::presented_certificates(server).await?
        };
        let Some(own) = chain.into_iter().next() else {
            println!("no certificate");
            continue;
        };
        let seen = presented.iter().any(|kept| kept.as_ref() == own.as_slice());
        println!(
            "SHA-256 {}{}",
            tls::fingerprint(&own),
            if seen { " (the same)" } else { "" }
        );
        if !seen {
            presented.push(CertificateDer::from(own));
        }
    }
    if presented.is_empty() {
        bail!("{name}'s servers presented no certificate to trust");
    }
    println!(
        "\nTrusted as they are, these are the only certificates {name}'s servers may present. Compare with the \
         fingerprints the server says its own are (for Proton Mail Bridge, export its certificate, then \
         `openssl x509 -noout -fingerprint -sha256 -in cert.pem`)."
    );
    if !confirm(&format!(
        "Trust {} for {name}? [y/N] ",
        if presented.len() == 1 { "it" } else { "them" }
    ))? {
        bail!("nothing trusted");
    }
    let directory = xdg::config_dir()
        .context("there's no configuration directory: neither XDG_CONFIG_HOME nor HOME is set")?
        .join("certificates");
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let path = directory.join(format!("{name}.pem"));
    std::fs::write(&path, tls::pem(&presented))
        .with_context(|| format!("writing {}", path.display()))?;
    println!("Kept in {}.", path.display());
    let config_path =
        Config::path().map_or_else(|| "config.toml".into(), |path| path.display().to_string());
    match &account.trust {
        Trust::Pinned(named) if *named == path => println!("{name} trusts it already."),
        Trust::Roots { ca_file: Some(_) } => println!(
            "To trust it, replace ca_file with this, in the account {name} of {config_path}:\n\n    \
             server_certificate = \"{}\"",
            path.display()
        ),
        _ => println!(
            "To trust it, add this to the account {name} of {config_path}:\n\n    server_certificate = \"{}\"",
            path.display()
        ),
    }
    Ok(())
}

/// Whether the user answers `prompt` yes.
fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
