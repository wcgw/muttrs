//! Sending over SMTP submission, by lettre: implicit TLS or STARTTLS, signing in with a password (PLAIN, LOGIN) or
//! XOAUTH2.
//!
//! lettre takes no TLS settings of muttrs' own, so a server whose certificate is trusted as it is
//! ([`Trust::Pinned`]) is connected to without verifying its certificate, then turned away before signing in unless
//! the certificate it presented is one of those trusted. The handshake still checks the server holds that
//! certificate's key: lettre's verifier skips the certificate's checks, not the handshake's signatures.

use std::time::Duration;

use lettre::transport::smtp::authentication::{Credentials as SmtpCredentials, Mechanism};
use lettre::transport::smtp::client::{AsyncSmtpConnection, Certificate, TlsParameters};
use lettre::transport::smtp::extension::ClientId;

use crate::account::{Endpoint, Tls};
use crate::compose::Built;
use crate::imap::Credentials;
use crate::tls::{self, Trust};

/// How long connecting, or sending, may take, before giving up.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Why a Message wasn't sent.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The certificates to trust can't be read.
    #[error(transparent)]
    Certificates(#[from] tls::Error),
    /// The sender or a recipient isn't an address SMTP takes.
    #[error("{0:?} isn't an address SMTP takes")]
    Address(String),
    /// The server's certificate isn't the one trusted.
    #[error(
        "the certificate {0} presents isn't trusted: it isn't one of server_certificate's, but SHA-256 {1}. If it's \
         the server's new one, `muttrs trust ACCOUNT` keeps it"
    )]
    NotPinned(String, String),
    /// The server couldn't be reached, refused TLS or the credentials, or refused the Message.
    #[error("sending through {0}: {1}")]
    Smtp(String, String),
}

/// Sends `built` through `server`, trusting its certificate as `trust` says.
///
/// # Errors
///
/// If the server can't be reached, its certificate isn't trusted, signing in fails, or the server refuses the
/// Message.
pub async fn send(
    server: &Endpoint,
    trust: &Trust,
    credentials: &Credentials,
    built: &Built,
) -> Result<(), Error> {
    let failed = |error: &dyn std::fmt::Display| Error::Smtp(server.to_string(), error.to_string());
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
    let address = |text: &str| text.parse().map_err(|_| Error::Address(text.to_owned()));
    let recipients = built
        .recipients
        .iter()
        .map(|recipient| address(recipient))
        .collect::<Result<Vec<_>, _>>()?;
    let envelope = lettre::address::Envelope::new(Some(address(&built.from)?), recipients)
        .map_err(|error| failed(&error))?;
    let sending = async {
        let mut connection = connect(server, trust).await?;
        connection
            .auth(&mechanisms, &credentials)
            .await
            .map_err(|error| failed(&error))?;
        connection
            .send(&envelope, &built.message)
            .await
            .map_err(|error| failed(&error))?;
        let _ = connection.quit().await;
        Ok(())
    };
    tokio::time::timeout(TIMEOUT, sending)
        .await
        .map_err(|_| failed(&"it took too long"))?
}

/// A connection to `server`, secured as it says, its certificate trusted as `trust` says.
async fn connect(server: &Endpoint, trust: &Trust) -> Result<AsyncSmtpConnection, Error> {
    let failed = |error: &dyn std::fmt::Display| Error::Smtp(server.to_string(), error.to_string());
    let mut parameters = TlsParameters::builder(server.host.clone());
    let pinned = match trust {
        Trust::Roots { ca_file } => {
            if let Some(path) = ca_file {
                for certificate in tls::certificates(path)? {
                    let certificate = Certificate::from_der(certificate.to_vec())
                        .map_err(|error| failed(&error))?;
                    parameters = parameters.add_root_certificate(certificate);
                }
            }
            None
        }
        Trust::Pinned(path) => {
            parameters = parameters
                .dangerous_accept_invalid_certs(true)
                .dangerous_accept_invalid_hostnames(true);
            Some(tls::certificates(path)?)
        }
    };
    let connection = open(
        server,
        parameters.build_rustls().map_err(|error| failed(&error))?,
    )
    .await?;
    if let (Some(pinned), Tls::Ssl | Tls::Starttls) = (pinned, server.tls) {
        let presented = connection
            .peer_certificate()
            .map_err(|error| failed(&error))?;
        if !pinned
            .iter()
            .any(|certificate| certificate.as_ref() == presented.as_slice())
        {
            let mut connection = connection;
            connection.abort().await;
            return Err(Error::NotPinned(
                server.host.clone(),
                tls::fingerprint(&presented),
            ));
        }
    }
    Ok(connection)
}

/// Connects to `server`, securing the connection with `parameters` as it says, up to signing in.
async fn open(server: &Endpoint, parameters: TlsParameters) -> Result<AsyncSmtpConnection, Error> {
    let failed = |error: &dyn std::fmt::Display| Error::Smtp(server.to_string(), error.to_string());
    let hello = ClientId::default();
    let address = (server.host.as_str(), server.port);
    let implicit = (server.tls == Tls::Ssl).then(|| parameters.clone());
    let mut connection =
        AsyncSmtpConnection::connect_tokio1(address, Some(TIMEOUT), &hello, implicit, None)
            .await
            .map_err(|error| failed(&error))?;
    if server.tls == Tls::Starttls {
        if !connection.can_starttls() {
            connection.abort().await;
            return Err(failed(
                &"it doesn't offer STARTTLS, and muttrs won't sign in unencrypted",
            ));
        }
        connection
            .starttls(parameters, &hello)
            .await
            .map_err(|error| failed(&error))?;
    }
    Ok(connection)
}

/// The certificates `server` presents, end entity first, without trusting them and without signing in: what
/// `muttrs trust` shows before keeping them. None over a connection without TLS.
///
/// # Errors
///
/// If the server can't be reached, or TLS fails for another reason than the certificate.
pub async fn presented_certificates(server: &Endpoint) -> Result<Vec<Vec<u8>>, Error> {
    if server.tls == Tls::None {
        return Ok(Vec::new());
    }
    let parameters = TlsParameters::builder(server.host.clone())
        .dangerous_accept_invalid_certs(true)
        .dangerous_accept_invalid_hostnames(true)
        .build_rustls()
        .map_err(|error| Error::Smtp(server.to_string(), error.to_string()))?;
    let mut connection = open(server, parameters).await?;
    let presented = connection
        .certificate_chain()
        .map_err(|error| Error::Smtp(server.to_string(), error.to_string()));
    let _ = connection.quit().await;
    presented
}
