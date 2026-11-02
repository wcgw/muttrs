//! An IMAP session with an Account's server: connecting over TLS or STARTTLS, signing in with a password or
//! XOAUTH2, and the commands muttrs needs.

use std::sync::Arc;
use std::time::Duration;

use async_imap::types::{Fetch, NameAttribute};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::TryStreamExt as _;
use jiff::Timestamp;
use rustls::ClientConfig;
use rustls::pki_types::CertificateDer;
use tokio::net::TcpStream;

use crate::account::{Endpoint, Tls};
use crate::envelope::{self, Envelope, Flags};
use crate::mailbox::{Counts, Mailbox};
use crate::tls::{self, Stream};

/// How long connecting may take, before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Why talking to the server failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// When the server, at this address, can't be reached or its greeting can't be read: why.
    #[error("connecting to {0}: {1}")]
    Connect(String, String),
    /// When the connection can't be secured with TLS.
    #[error(transparent)]
    Tls(#[from] tls::Error),
    /// When the server at this address refuses STARTTLS.
    #[error("{0} doesn't offer STARTTLS, and muttrs won't sign in unencrypted")]
    NoStarttls(String),
    /// When the server at this address turns the credentials down: why.
    #[error("signing in to {0}: {1}")]
    SignIn(String, String),
    /// When a command fails, or the connection does.
    #[error("IMAP: {0}")]
    Protocol(#[from] async_imap::error::Error),
    /// When the server leaves out something muttrs needs: its greeting, a Mailbox's UIDVALIDITY, a Message's body.
    #[error("the server sent no {0}")]
    Missing(&'static str),
}

impl Error {
    /// Whether the server turned the credentials down, so that trying again with them is pointless.
    #[must_use]
    pub fn is_sign_in(&self) -> bool {
        matches!(self, Self::SignIn(..))
    }

    /// Whether the connection is lost or unusable, rather than one command refused, so that the session must be
    /// made again.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        !matches!(
            self,
            Self::Missing(_)
                | Self::Protocol(
                    async_imap::error::Error::No(_)
                        | async_imap::error::Error::Bad(_)
                        | async_imap::error::Error::Validate(_)
                )
        )
    }
}

/// What to sign in with.
#[derive(Clone)]
pub enum Credentials {
    /// A password, for LOGIN.
    Password {
        /// The user name to sign in with.
        username: String,
        /// The user's password.
        password: String,
    },
    /// An OAuth2 access token, for XOAUTH2.
    Bearer {
        /// The user name to sign in with.
        username: String,
        /// The access token.
        token: String,
    },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password { username, .. } => write!(f, "Password({username}, …)"),
            Self::Bearer { username, .. } => write!(f, "Bearer({username}, …)"),
        }
    }
}

/// XOAUTH2's single message, then an empty one should the server explain a refusal.
struct XOAuth2 {
    message: Option<String>,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> String {
        self.message.take().unwrap_or_default()
    }
}

/// The XOAUTH2 message for `username` and an access token, before base64.
pub(crate) fn xoauth2(username: &str, token: &str) -> String {
    format!("user={username}\x01auth=Bearer {token}\x01\x01")
}

/// A Mailbox opened: what's in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selected {
    /// How many Messages it has.
    pub exists: u32,
    /// Its UIDVALIDITY: its Messages' UIDs stand as long as it does.
    pub uid_validity: u32,
    /// The lowest UID a Message added to it can get (UIDNEXT), if the server says.
    pub uid_next: Option<u32>,
}

/// A session with an Account's IMAP server, signed in, and what the server can do.
pub struct Session {
    // async_imap's Session, which this one wraps: no other name says so as plainly.
    #[allow(clippy::struct_field_names)]
    session: async_imap::Session<Stream>,
    /// Whether the server has MOVE (RFC 6851).
    can_move: bool,
    /// Whether the server has UIDPLUS (RFC 4315), for UID EXPUNGE.
    can_uid_expunge: bool,
    can_idle: bool,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("can_move", &self.can_move)
            .field("can_idle", &self.can_idle)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Connects to `server` and signs in.
    ///
    /// # Errors
    ///
    /// If the server can't be reached, TLS fails, or the credentials are refused.
    pub async fn connect(
        server: &Endpoint,
        tls_config: Arc<ClientConfig>,
        credentials: &Credentials,
    ) -> Result<Self, Error> {
        let address = format!("{}:{}", server.host, server.port);
        let client = open(server, tls_config).await?;
        let session = match credentials {
            Credentials::Password { username, password } => client
                .login(username, password)
                .await
                .map_err(|(error, _)| Error::SignIn(address.clone(), error.to_string()))?,
            Credentials::Bearer { username, token } => {
                let message = xoauth2(username, token);
                // The message on the command's line (SASL-IR, RFC 4959), as Gmail, Outlook and Dovecot take it and
                // GreenMail needs it; then, should the server not take that, after its continuation. The empty
                // response either way answers a refusal's explanation.
                let initial = format!("XOAUTH2 {}", BASE64.encode(&message));
                match client
                    .authenticate(initial, XOAuth2 { message: None })
                    .await
                {
                    Ok(session) => session,
                    Err((async_imap::error::Error::Bad(_), client)) => client
                        .authenticate(
                            "XOAUTH2",
                            XOAuth2 {
                                message: Some(message),
                            },
                        )
                        .await
                        .map_err(|(error, _)| Error::SignIn(address.clone(), error.to_string()))?,
                    Err((error, _)) => {
                        return Err(Error::SignIn(address.clone(), error.to_string()));
                    }
                }
            }
        };
        let mut session = Self {
            session,
            can_move: false,
            can_uid_expunge: false,
            can_idle: false,
        };
        let capabilities = session.session.capabilities().await?;
        session.can_move = capabilities.has_str("MOVE");
        session.can_uid_expunge = capabilities.has_str("UIDPLUS");
        session.can_idle = capabilities.has_str("IDLE");
        Ok(session)
    }

    /// Whether the server has IDLE (RFC 2177), so that [`Session::idle`] can wait for changes.
    #[must_use]
    pub fn can_idle(&self) -> bool {
        self.can_idle
    }

    /// Every Mailbox of the Account.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn mailboxes(&mut self) -> Result<Vec<Mailbox>, Error> {
        let names: Vec<_> = self
            .session
            .list(Some(""), Some("*"))
            .await?
            .try_collect()
            .await?;
        Ok(names
            .iter()
            .map(|name| {
                let attributes: Vec<String> = name
                    .attributes()
                    .iter()
                    .map(|attribute| match attribute {
                        NameAttribute::NoInferiors => "\\Noinferiors".to_owned(),
                        NameAttribute::NoSelect => "\\Noselect".to_owned(),
                        NameAttribute::Marked => "\\Marked".to_owned(),
                        NameAttribute::Unmarked => "\\Unmarked".to_owned(),
                        NameAttribute::All => "\\All".to_owned(),
                        NameAttribute::Archive => "\\Archive".to_owned(),
                        NameAttribute::Drafts => "\\Drafts".to_owned(),
                        NameAttribute::Flagged => "\\Flagged".to_owned(),
                        NameAttribute::Junk => "\\Junk".to_owned(),
                        NameAttribute::Sent => "\\Sent".to_owned(),
                        NameAttribute::Trash => "\\Trash".to_owned(),
                        NameAttribute::Extension(other) => other.to_string(),
                        _ => String::new(),
                    })
                    .collect();
                let attributes: Vec<&str> = attributes.iter().map(String::as_str).collect();
                Mailbox::listed(name.name(), name.delimiter(), &attributes)
            })
            .collect())
    }

    /// How many Messages `mailbox` has, and how many are Unread.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn counts(&mut self, mailbox: &str) -> Result<Counts, Error> {
        let status = self.session.status(mailbox, "(MESSAGES UNSEEN)").await?;
        Ok(Counts {
            messages: status.exists,
            unread: status.unseen.unwrap_or(0),
        })
    }

    /// How many Messages of the open Mailbox are Unread.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn unread(&mut self) -> Result<u32, Error> {
        let unseen = self.session.uid_search("UNSEEN").await?;
        Ok(u32::try_from(unseen.len()).unwrap_or(u32::MAX))
    }

    /// The UIDs of the open Mailbox's Messages that `query` finds: Gmail's own search syntax on Gmail
    /// (`X-GM-RAW`), their text (`TEXT`, headers and bodies) elsewhere.
    ///
    /// # Errors
    ///
    /// If the server fails the command, as when it can't search that, or the connection.
    pub async fn search(&mut self, query: &str, gmail: bool) -> Result<Vec<u32>, Error> {
        let criterion = if gmail { "X-GM-RAW" } else { "TEXT" };
        let quoted = format!("\"{}\"", query.replace('\\', "\\\\").replace('"', "\\\""));
        // Strictly, a quoted string is ASCII; servers that take UTF-8 at all take it so, said to be.
        let charset = if query.is_ascii() {
            ""
        } else {
            "CHARSET UTF-8 "
        };
        let mut uids: Vec<u32> = self
            .session
            .uid_search(format!("{charset}{criterion} {quoted}"))
            .await?
            .into_iter()
            .collect();
        uids.sort_unstable();
        Ok(uids)
    }

    /// Opens `mailbox`, to read and change its Messages.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn select(&mut self, mailbox: &str) -> Result<Selected, Error> {
        let selected = self.session.select(mailbox).await?;
        Ok(Selected {
            exists: selected.exists,
            // A server that doesn't say (GreenMail's response lacks the text RFC 3501 wants after the code, so it
            // isn't read) is taken never to renumber its Messages.
            uid_validity: selected.uid_validity.unwrap_or_else(|| {
                tracing::debug!(mailbox, "no UIDVALIDITY: taken as 0");
                0
            }),
            uid_next: selected.uid_next,
        })
    }

    /// The Envelopes of the newest `count` Messages of the open Mailbox, which has `exists`.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn newest(&mut self, exists: u32, count: u32) -> Result<Vec<Envelope>, Error> {
        if exists == 0 || count == 0 {
            return Ok(Vec::new());
        }
        let first = exists.saturating_sub(count - 1).max(1);
        let fetches: Vec<Fetch> = self
            .session
            .fetch(format!("{first}:{exists}"), envelope_query())
            .await?
            .try_collect()
            .await?;
        Ok(fetches.iter().filter_map(read_envelope).collect())
    }

    /// The Envelopes of the Messages of the open Mailbox from sequence number `first` to `last`, both included.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn between(&mut self, first: u32, last: u32) -> Result<Vec<Envelope>, Error> {
        if first == 0 || last < first {
            return Ok(Vec::new());
        }
        let fetches: Vec<Fetch> = self
            .session
            .fetch(format!("{first}:{last}"), envelope_query())
            .await?
            .try_collect()
            .await?;
        Ok(fetches.iter().filter_map(read_envelope).collect())
    }

    /// The sequence number of the open Mailbox's Message `uid`, if it has it.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn sequence_of(&mut self, uid: u32) -> Result<Option<u32>, Error> {
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uid.to_string(), "(UID)")
            .await?
            .try_collect()
            .await?;
        Ok(fetches
            .iter()
            .find(|fetch| fetch.uid == Some(uid))
            .map(|fetch| fetch.message))
    }

    /// The Envelopes of the Messages of the open Mailbox whose UIDs are in `uids`, a UID set (`5:*`).
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn envelopes(&mut self, uids: &str) -> Result<Vec<Envelope>, Error> {
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uids, envelope_query())
            .await?
            .try_collect()
            .await?;
        Ok(fetches.iter().filter_map(read_envelope).collect())
    }

    /// The Flags of the Messages of the open Mailbox whose UIDs are in `uids`, by UID.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn flags(&mut self, uids: &str) -> Result<Vec<(u32, Flags)>, Error> {
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uids, "(UID FLAGS)")
            .await?
            .try_collect()
            .await?;
        Ok(fetches
            .iter()
            .filter_map(|fetch| Some((fetch.uid?, flags_of(fetch))))
            .collect())
    }

    /// The whole of Message `uid` of the open Mailbox, without marking it Seen.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection, or has no such Message.
    pub async fn body(&mut self, uid: u32) -> Result<Vec<u8>, Error> {
        let fetches: Vec<Fetch> = self
            .session
            .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
            .await?
            .try_collect()
            .await?;
        fetches
            .iter()
            .find(|fetch| fetch.uid == Some(uid))
            .and_then(Fetch::body)
            .map(<[u8]>::to_vec)
            .ok_or(Error::Missing("message body"))
    }

    /// Adds `flag` (as `\Seen`) to the Messages `uids` of the open Mailbox, or takes it off.
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn set_flag(&mut self, uids: &[u32], flag: &str, on: bool) -> Result<(), Error> {
        let sign = if on { '+' } else { '-' };
        let _: Vec<Fetch> = self
            .session
            .uid_store(uid_set(uids), format!("{sign}FLAGS.SILENT ({flag})"))
            .await?
            .try_collect()
            .await?;
        Ok(())
    }

    /// Moves the Messages `uids` of the open Mailbox to `target`: by MOVE where the server has it, or else by
    /// COPY, then marking them Deleted and expunging them.
    ///
    /// # Errors
    ///
    /// If the server fails a command, or the connection.
    pub async fn move_to(&mut self, uids: &[u32], target: &str) -> Result<(), Error> {
        let set = uid_set(uids);
        if self.can_move {
            self.session.uid_mv(&set, target).await?;
            return Ok(());
        }
        self.session.uid_copy(&set, target).await?;
        self.expunge(uids).await
    }

    /// Marks the Messages `uids` of the open Mailbox Deleted, and expunges them: only them, where the server has
    /// UID EXPUNGE, or every Message marked Deleted otherwise.
    ///
    /// # Errors
    ///
    /// If the server fails a command, or the connection.
    pub async fn expunge(&mut self, uids: &[u32]) -> Result<(), Error> {
        let set = uid_set(uids);
        self.set_flag(uids, "\\Deleted", true).await?;
        if self.can_uid_expunge {
            let _: Vec<_> = self.session.uid_expunge(&set).await?.try_collect().await?;
        } else {
            let _: Vec<_> = self.session.expunge().await?.try_collect().await?;
        }
        Ok(())
    }

    /// Creates `mailbox`.
    ///
    /// # Errors
    ///
    /// If the server fails the command (it exists already, say), or the connection.
    pub async fn create(&mut self, mailbox: &str) -> Result<(), Error> {
        self.session.create(mailbox).await?;
        Ok(())
    }

    /// Adds `message` to `mailbox`, with `flags` (as `(\Seen)`).
    ///
    /// # Errors
    ///
    /// If the server fails the command, or the connection.
    pub async fn append(
        &mut self,
        mailbox: &str,
        flags: &str,
        message: &[u8],
    ) -> Result<(), Error> {
        self.session
            .append(mailbox, Some(flags), None, message)
            .await?;
        Ok(())
    }

    /// Waits until the open Mailbox changes, or `timeout` passes, by IDLE: `true` if it changed.
    ///
    /// # Errors
    ///
    /// If the connection fails.
    pub async fn idle(self, timeout: Duration) -> Result<(Self, bool), Error> {
        let Self {
            session,
            can_move,
            can_uid_expunge,
            can_idle,
        } = self;
        let mut handle = session.idle();
        handle.init().await?;
        let (waiting, _interrupt) = handle.wait_with_timeout(timeout);
        let changed = matches!(
            waiting.await?,
            async_imap::extensions::idle::IdleResponse::NewData(_)
        );
        let session = handle.done().await?;
        Ok((
            Self {
                session,
                can_move,
                can_uid_expunge,
                can_idle,
            },
            changed,
        ))
    }

    /// Signs out, closing the connection.
    pub async fn logout(mut self) {
        let _ = self.session.logout().await;
    }
}

/// Connects to `server`, securing the connection as it says, up to signing in.
async fn open(
    server: &Endpoint,
    tls_config: Arc<ClientConfig>,
) -> Result<async_imap::Client<Stream>, Error> {
    let address = format!("{}:{}", server.host, server.port);
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address))
        .await
        .map_err(|_| Error::Connect(address.clone(), "timed out".into()))?
        .map_err(|error| Error::Connect(address.clone(), error.to_string()))?;
    Ok(match server.tls {
        Tls::Ssl => {
            let stream = tls::wrap(tcp, &server.host, tls_config).await?;
            let mut client = async_imap::Client::new(Stream::Tls(Box::new(stream)));
            greeting(&mut client).await?;
            client
        }
        Tls::Starttls => {
            let mut client = async_imap::Client::new(Stream::Plain(tcp));
            greeting(&mut client).await?;
            client
                .run_command_and_check_ok("STARTTLS", None)
                .await
                .map_err(|_| Error::NoStarttls(address.clone()))?;
            let Stream::Plain(tcp) = client.into_inner() else {
                unreachable!("the stream is plain until STARTTLS")
            };
            let stream = tls::wrap(tcp, &server.host, tls_config).await?;
            // No greeting after STARTTLS: the session goes on.
            async_imap::Client::new(Stream::Tls(Box::new(stream)))
        }
        Tls::None => {
            let mut client = async_imap::Client::new(Stream::Plain(tcp));
            greeting(&mut client).await?;
            client
        }
    })
}

/// The certificates `server` presents, end entity first, without trusting them and without signing in: what
/// `muttrs trust` shows before keeping them. None over a connection without TLS.
///
/// # Errors
///
/// If the server can't be reached, or TLS fails for another reason than the certificate.
pub async fn presented_certificates(
    server: &Endpoint,
) -> Result<Vec<CertificateDer<'static>>, Error> {
    let (config, presented) = tls::recording_config();
    drop(open(server, config).await?);
    let presented = presented
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    Ok(presented)
}

/// Reads the server's greeting.
async fn greeting(client: &mut async_imap::Client<Stream>) -> Result<(), Error> {
    client
        .read_response()
        .await
        .map_err(|error| Error::Connect("the server".into(), error.to_string()))?
        .ok_or(Error::Missing("greeting"))?;
    Ok(())
}

fn envelope_query() -> String {
    format!(
        "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS ({})])",
        envelope::FIELDS
    )
}

fn flags_of(fetch: &Fetch) -> Flags {
    let names: Vec<String> = fetch.flags().map(|flag| flag_name(&flag)).collect();
    Flags::named(names.iter().map(String::as_str))
}

fn flag_name(flag: &async_imap::types::Flag<'_>) -> String {
    use async_imap::types::Flag;
    match flag {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(name) => name.to_string(),
    }
}

fn read_envelope(fetch: &Fetch) -> Option<Envelope> {
    let uid = fetch.uid?;
    let received = fetch
        .internal_date()
        .and_then(|date| Timestamp::from_second(date.timestamp()).ok());
    Some(Envelope::read(
        uid,
        flags_of(fetch),
        received,
        fetch.size.unwrap_or(0),
        fetch.header().unwrap_or_default(),
    ))
}

/// UIDs as a UID set, runs of consecutive UIDs as ranges: `1:3,7`.
pub fn uid_set(uids: &[u32]) -> String {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < sorted.len() {
        let start = sorted[index];
        let mut end = start;
        while index + 1 < sorted.len() && sorted[index + 1] == end + 1 {
            index += 1;
            end = sorted[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        index += 1;
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_sets_make_ranges() {
        assert_eq!(uid_set(&[7, 1, 2, 3, 9, 10, 3]), "1:3,7,9:10");
        assert_eq!(uid_set(&[5]), "5");
    }

    #[test]
    fn xoauth2_messages() {
        assert_eq!(
            xoauth2("ann@gmail.com", "ya29.token"),
            "user=ann@gmail.com\x01auth=Bearer ya29.token\x01\x01"
        );
    }
}
