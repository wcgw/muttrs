//! Sending over SMTP submission, by lettre: implicit TLS or STARTTLS, signing in with a password (PLAIN, LOGIN) or
//! XOAUTH2.

use std::path::Path;
use std::time::Duration;

use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::transport::smtp::client::{Certificate, Tls as SmtpTls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};

use crate::account::{Endpoint, Tls};
use crate::compose::Built;
use crate::imap::Credentials;

/// How long sending may take, before giving up.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Why a Message wasn't sent.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The CA file named for the Account can't be read.
    #[error("reading the CA file {0}: {1}")]
    CaFile(String, String),
    /// The sender or a recipient isn't an address SMTP takes.
    #[error("{0:?} isn't an address SMTP takes")]
    Address(String),
    /// The server couldn't be reached, refused TLS or the credentials, or refused the Message.
    #[error("sending through {0}: {1}")]
    Smtp(String, String),
}

/// Sends `built` through `server`, trusting the certificates in `ca_file` besides the platform's.
///
/// # Errors
///
/// If the server can't be reached, TLS fails, signing in fails, or the server refuses the Message.
pub async fn send(
    server: &Endpoint,
    ca_file: Option<&Path>,
    credentials: &Credentials,
    built: &Built,
) -> Result<(), Error> {
    let failed = |error: &dyn std::fmt::Display| Error::Smtp(server.to_string(), error.to_string());
    let mut parameters = TlsParameters::builder(server.host.clone());
    if let Some(path) = ca_file {
        let pem = std::fs::read(path)
            .map_err(|error| Error::CaFile(path.display().to_string(), error.to_string()))?;
        let certificate = Certificate::from_pem(&pem)
            .map_err(|error| Error::CaFile(path.display().to_string(), error.to_string()))?;
        parameters = parameters.add_root_certificate(certificate);
    }
    let parameters = parameters.build_rustls().map_err(|error| failed(&error))?;
    let tls = match server.tls {
        Tls::Ssl => SmtpTls::Wrapper(parameters),
        Tls::Starttls => SmtpTls::Required(parameters),
        Tls::None => SmtpTls::None,
    };
    let (credentials, mechanisms) = match credentials {
        Credentials::Password { username, password } => (
            SmtpCredentials::new(username.clone(), password.clone()),
            vec![Mechanism::Plain, Mechanism::Login],
        ),
        Credentials::Bearer { username, token } => (
            SmtpCredentials::new(username.clone(), token.clone()),
            vec![Mechanism::Xoauth2],
        ),
    };
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&server.host)
        .port(server.port)
        .tls(tls)
        .credentials(credentials)
        .authentication(mechanisms)
        .timeout(Some(TIMEOUT))
        .build();
    let address = |text: &str| text.parse().map_err(|_| Error::Address(text.to_owned()));
    let recipients = built
        .recipients
        .iter()
        .map(|recipient| address(recipient))
        .collect::<Result<Vec<_>, _>>()?;
    let envelope = lettre::address::Envelope::new(Some(address(&built.from)?), recipients)
        .map_err(|error| failed(&error))?;
    transport
        .send_raw(&envelope, &built.message)
        .await
        .map_err(|error| failed(&error))?;
    Ok(())
}
