//! TLS, by rustls with the ring provider. A server's certificate is trusted by the platform's root certificates
//! (and those of a CA file, when an Account names one), or, for a server with a certificate of its own such as a
//! mail bridge's, by being exactly a certificate the Account names.

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, OtherError, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// How an Account's servers' certificates are trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// By the platform's root certificates, and those of a CA file if there is one: certificates a CA signed for
    /// the server's name.
    Roots {
        /// A PEM file of CA certificates to trust besides the platform's.
        ca_file: Option<PathBuf>,
    },
    /// By being one of the certificates in this PEM file, exactly, whatever it says of itself: who signed it, the
    /// names it's for, when it expires. The server still proves it holds the certificate's key.
    Pinned(PathBuf),
}

impl Default for Trust {
    fn default() -> Self {
        Self::Roots { ca_file: None }
    }
}

/// Why TLS can't be set up, or failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// When there's no certificate to trust: the platform has none, and no CA file is named.
    #[error("no root certificates to trust: neither the platform's nor a CA file's")]
    NoRoots,
    /// When a file of certificates (by its path) can't be read, holds none, or holds one that's no good: why.
    #[error("reading the certificates in {0}: {1}")]
    Certificates(String, String),
    /// When a host isn't a name, or an address, TLS can verify.
    #[error("{0} isn't a host name TLS can verify")]
    Name(String),
    /// When the server's certificate isn't trusted, and why.
    #[error(
        "the certificate {host} presents isn't trusted: {why}. If the server is one you trust (your own, or a mail \
         bridge), `muttrs trust ACCOUNT` keeps its certificate, to trust as it is"
    )]
    Untrusted {
        /// The host whose certificate it is.
        host: String,
        /// Why it isn't trusted.
        why: String,
    },
    /// When the handshake fails for another reason than the certificate.
    #[error("TLS with {host}: {error}")]
    Handshake {
        /// The host it was with.
        host: String,
        /// Why it failed.
        error: io::Error,
    },
}

/// The certificates in the PEM file at `path`.
///
/// # Errors
///
/// If it can't be read, or holds no certificate.
pub fn certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, Error> {
    let failed = |error: &dyn std::fmt::Display| {
        Error::Certificates(path.display().to_string(), error.to_string())
    };
    let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
        .map_err(|error| failed(&error))?
        .collect::<Result<_, _>>()
        .map_err(|error| failed(&error))?;
    if certificates.is_empty() {
        return Err(failed(&"it holds no certificate"));
    }
    Ok(certificates)
}

/// A certificate's SHA-256 fingerprint, as `AB:CD:…`, which is how browsers and `openssl x509 -fingerprint` show it.
#[must_use]
pub fn fingerprint(certificate: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, certificate)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// `certificates` as a PEM file says them.
#[must_use]
pub fn pem(certificates: &[CertificateDer<'_>]) -> String {
    use base64::Engine as _;
    let mut pem = String::new();
    for certificate in certificates {
        pem.push_str("-----BEGIN CERTIFICATE-----\n");
        let encoded = base64::engine::general_purpose::STANDARD.encode(certificate.as_ref());
        for line in encoded.as_bytes().chunks(64) {
            pem.push_str(&String::from_utf8_lossy(line));
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
    }
    pem
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The TLS settings every connection of an Account uses, trusting as `trust` says.
///
/// # Errors
///
/// If there are no certificates to trust, or a file of them can't be read.
///
/// # Panics
///
/// If ring doesn't support rustls' default protocol versions, which it does.
pub fn client_config(trust: &Trust) -> Result<Arc<ClientConfig>, Error> {
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions");
    let ca_file = match trust {
        Trust::Pinned(path) => {
            let verifier = Pinned {
                certificates: certificates(path)?,
                algorithms: provider().signature_verification_algorithms,
            };
            return Ok(Arc::new(
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(verifier))
                    .with_no_client_auth(),
            ));
        }
        Trust::Roots { ca_file } => ca_file.as_deref(),
    };
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for error in &native.errors {
        tracing::debug!(%error, "a platform certificate couldn't be loaded");
    }
    let (added, _) = roots.add_parsable_certificates(native.certs);
    if let Some(path) = ca_file {
        for certificate in certificates(path)? {
            roots.add(certificate).map_err(|error| {
                Error::Certificates(path.display().to_string(), error.to_string())
            })?;
        }
    } else if added == 0 {
        return Err(Error::NoRoots);
    }
    Ok(Arc::new(
        builder.with_root_certificates(roots).with_no_client_auth(),
    ))
}

/// TLS settings that take any certificate, keeping those presented in the returned list, end entity first: to see
/// what a server presents, before trusting it. Never use them to sign in.
///
/// # Panics
///
/// If ring doesn't support rustls' default protocol versions, which it does.
#[must_use]
pub fn recording_config() -> (Arc<ClientConfig>, Arc<Mutex<Vec<CertificateDer<'static>>>>) {
    let presented = Arc::new(Mutex::new(Vec::new()));
    let verifier = Recording {
        presented: Arc::clone(&presented),
        algorithms: provider().signature_verification_algorithms,
    };
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    (Arc::new(config), presented)
}

/// Trusts the certificates it has, and no other, still checking the handshake proves the server holds the key.
#[derive(Debug)]
struct Pinned {
    certificates: Vec<CertificateDer<'static>>,
    algorithms: WebPkiSupportedAlgorithms,
}

/// Why a [`Pinned`] verifier turned a certificate down.
#[derive(Debug, thiserror::Error)]
#[error("it isn't one of server_certificate's, but SHA-256 {0}")]
struct NotPinned(String);

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if self
            .certificates
            .iter()
            .any(|pinned| pinned.as_ref() == end_entity.as_ref())
        {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                OtherError(Arc::new(NotPinned(fingerprint(end_entity)))),
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, certificate, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, certificate, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Takes any certificate, keeping what's presented.
#[derive(Debug)]
struct Recording {
    presented: Arc<Mutex<Vec<CertificateDer<'static>>>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Recording {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut presented = self
            .presented
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        presented.clear();
        presented.push(end_entity.clone().into_owned());
        presented.extend(
            intermediates
                .iter()
                .map(|certificate| certificate.clone().into_owned()),
        );
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, certificate, signature, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, certificate, signature, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// Why a certificate wasn't trusted, in words.
fn untrusted(error: &CertificateError) -> String {
    match error {
        CertificateError::UnknownIssuer => "no CA muttrs trusts signed it".into(),
        CertificateError::Expired | CertificateError::ExpiredContext { .. } => "it has expired".into(),
        CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
            "it's for another name".into()
        }
        CertificateError::Other(OtherError(other)) => match other.downcast_ref::<NotPinned>() {
            Some(not_pinned) => not_pinned.to_string(),
            None if format!("{other:?}").contains("CaUsedAsEndEntity") => {
                "it's a CA's certificate, which a server can't present as its own (as self-signed ones often are)"
                    .into()
            }
            None => other.to_string(),
        },
        other => format!("{other:?}"),
    }
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
        .map_err(|error| {
            let certificate = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                .and_then(|inner| match inner {
                    rustls::Error::InvalidCertificate(certificate) => Some(untrusted(certificate)),
                    _ => None,
                });
            match certificate {
                Some(why) => Error::Untrusted {
                    host: host.to_owned(),
                    why,
                },
                None => Error::Handshake {
                    host: host.to_owned(),
                    error,
                },
            }
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

#[cfg(test)]
mod tests {
    use rustls::pki_types::PrivateKeyDer;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::*;

    /// A self-signed certificate for 127.0.0.1 that says it's a CA's, as mail bridges' do, and its key.
    fn bridge_certificate() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate().unwrap();
        let certificate = params.self_signed(&key).unwrap();
        (
            certificate.der().clone(),
            PrivateKeyDer::Pkcs8(key.serialize_der().into()),
        )
    }

    /// Serves TLS with `certificate` on a port of its own, answering "hello"; its address.
    async fn serve(certificate: CertificateDer<'static>, key: PrivateKeyDer<'static>) -> String {
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                if let Ok(mut stream) = acceptor.accept(tcp).await {
                    let _ = stream.write_all(b"hello").await;
                    let _ = stream.shutdown().await;
                }
            }
        });
        address
    }

    /// A PEM file holding `certificate`, in a directory of the test's own.
    fn pem_file(name: &str, certificate: &CertificateDer<'_>) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("muttrs-tls-{}-{name}.pem", std::process::id()));
        std::fs::write(&path, pem(std::slice::from_ref(certificate))).unwrap();
        path
    }

    async fn greeting(address: &str, trust: &Trust) -> Result<String, Error> {
        let tcp = TcpStream::connect(address).await.unwrap();
        let mut stream = wrap(tcp, "127.0.0.1", client_config(trust)?).await?;
        let mut said = String::new();
        stream.read_to_string(&mut said).await.unwrap();
        Ok(said)
    }

    #[tokio::test]
    async fn a_servers_own_ca_certificate_is_trusted_pinned_and_only_so() {
        let (certificate, key) = bridge_certificate();
        let address = serve(certificate.clone(), key).await;
        let own = pem_file("own", &certificate);

        // As a CA, it's refused, as webpki refuses a CA's certificate presented as a server's own.
        let as_ca = greeting(
            &address,
            &Trust::Roots {
                ca_file: Some(own.clone()),
            },
        )
        .await
        .unwrap_err();
        assert!(
            as_ca.to_string().contains("it's a CA's certificate"),
            "{as_ca}"
        );
        assert!(as_ca.to_string().contains("muttrs trust"), "{as_ca}");

        // As the server's own, it's trusted.
        assert_eq!(
            greeting(&address, &Trust::Pinned(own.clone()))
                .await
                .unwrap(),
            "hello"
        );

        // Another certificate pinned isn't this one.
        let (other, _) = bridge_certificate();
        let elsewhere = pem_file("other", &other);
        let refused = greeting(&address, &Trust::Pinned(elsewhere.clone()))
            .await
            .unwrap_err();
        assert!(
            refused.to_string().contains(&format!(
                "isn't one of server_certificate's, but SHA-256 {}",
                fingerprint(&certificate)
            )),
            "{refused}"
        );

        // What's presented, recorded without trusting it.
        let (config, presented) = recording_config();
        let tcp = TcpStream::connect(&address).await.unwrap();
        drop(wrap(tcp, "127.0.0.1", config).await.unwrap());
        assert_eq!(
            *presented.lock().unwrap(),
            std::slice::from_ref(&certificate)
        );

        for path in [own, elsewhere] {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn pem_round_trips() {
        let (certificate, _) = bridge_certificate();
        let path = pem_file("round-trip", &certificate);
        assert_eq!(
            certificates(&path).unwrap(),
            std::slice::from_ref(&certificate)
        );
        std::fs::remove_file(path).unwrap();
        assert_eq!(fingerprint(&certificate).len(), 32 * 3 - 1);
    }
}
