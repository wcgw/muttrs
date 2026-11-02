//! TLS, by rustls with the ring provider: the platform's root certificates, and those of a CA file when an Account
//! names one.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::ClientConfig;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// Why TLS can't be set up.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// When there's no certificate to trust: the platform has none, and no CA file is named.
    #[error("no root certificates to trust: neither the platform's nor a CA file's")]
    NoRoots,
    /// When the CA file at this path can't be read, holds no certificate, or holds one that's no good: why.
    #[error("reading the CA file {0}: {1}")]
    CaFile(String, String),
    /// When a host isn't a name, or an address, TLS can verify.
    #[error("{0} isn't a host name TLS can verify")]
    Name(String),
    /// When the handshake fails, as when the certificate isn't trusted or isn't the host's.
    #[error("TLS with {host}: {source}")]
    Handshake {
        /// The host it was with.
        host: String,
        /// Why it failed.
        source: io::Error,
    },
}

/// The TLS settings every connection of an Account uses.
///
/// # Errors
///
/// If there are no certificates to trust, or the CA file can't be read.
///
/// # Panics
///
/// If ring doesn't support rustls' default protocol versions, which it does.
pub fn client_config(ca_file: Option<&Path>) -> Result<Arc<ClientConfig>, Error> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        tracing::debug!(%error, "a platform certificate couldn't be loaded");
    }
    let (added, _) = roots.add_parsable_certificates(native.certs);
    if let Some(path) = ca_file {
        let failed = |error: &dyn std::fmt::Display| {
            Error::CaFile(path.display().to_string(), error.to_string())
        };
        let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
            .map_err(|error| failed(&error))?
            .collect::<Result<_, _>>()
            .map_err(|error| failed(&error))?;
        if certificates.is_empty() {
            return Err(failed(&"it holds no certificate"));
        }
        for certificate in certificates {
            roots.add(certificate).map_err(|error| failed(&error))?;
        }
    } else if added == 0 {
        return Err(Error::NoRoots);
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Secures `stream` with TLS, verifying the server is `host`.
///
/// # Errors
///
/// If the handshake fails, as when the certificate isn't trusted or isn't `host`'s.
pub async fn wrap(
    stream: TcpStream,
    host: &str,
    config: Arc<ClientConfig>,
) -> Result<TlsStream<TcpStream>, Error> {
    let name = ServerName::try_from(host.to_owned()).map_err(|_| Error::Name(host.to_owned()))?;
    TlsConnector::from(config)
        .connect(name, stream)
        .await
        .map_err(|source| Error::Handshake {
            host: host.to_owned(),
            source,
        })
}

/// A connection to a server, secured or not (to the loopback address only, for testing).
#[derive(Debug)]
pub enum Stream {
    /// Not secured: before STARTTLS, or to the loopback address.
    Plain(TcpStream),
    /// Secured with TLS.
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}
