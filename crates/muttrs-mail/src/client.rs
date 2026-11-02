//! An Account's client: a worker task that owns the IMAP session and does what the UI asks, reconnecting when the
//! connection drops, and a watcher that IDLEs on the Inbox for new mail. The UI never waits on the network: it
//! sends [`Request`]s and is told [`Update`]s.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::ClientConfig;
use tokio::sync::{Mutex, mpsc};

use crate::account::{Account, Auth, PasswordSource, Secret};
use crate::cache::{Cache, Listed};
use crate::compose::Built;
use crate::envelope::{Envelope, Flags};
use crate::imap::{self, Credentials, Session};
use crate::mailbox::{Counts, Mailbox, Role};
use crate::{oauth2, secret, smtp, tls};

/// How many of a Mailbox's newest Messages are listed when it's first opened.
const WINDOW: u32 = 300;
/// How long IDLE waits before starting again: Gmail drops it after about 30 minutes.
const IDLE_RENEWAL: Duration = Duration::from_mins(25);
/// How often a server without IDLE is asked what's new, and an idle session is kept alive.
const POLL: Duration = Duration::from_mins(5);
/// The longest wait before connecting again.
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// What the UI asks of an Account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// List its Mailboxes again, and connect again now if it's offline.
    Refresh,
    /// Open a Mailbox, and list its newest Messages' Envelopes.
    Open(String),
    /// List older Envelopes of a Mailbox than those listed: as many again as were listed first.
    More(String),
    /// Search a Mailbox on the server, for the Envelopes of the newest Messages found.
    Search {
        /// The Mailbox to search.
        mailbox: String,
        /// What to search for: Gmail's own syntax on Gmail, text anywhere in a Message elsewhere.
        query: String,
    },
    /// The whole of a Message.
    Body {
        /// The Mailbox it's in, by its name on the server.
        mailbox: String,
        /// Its UID there.
        uid: u32,
    },
    /// Set a Flag on Messages, or take it off.
    Flag {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
        /// The Flag to set or take off.
        flag: Flag,
        /// Whether to set it, rather than take it off.
        on: bool,
    },
    /// Move Messages to the Mailbox with a Role.
    MoveTo {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
        /// The Role of the Mailbox to move them to.
        role: Role,
    },
    /// Move Messages to another Mailbox, by its name.
    Move {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
        /// The Mailbox to move them to, by its name on the server.
        to: String,
    },
    /// Expunge Messages for good, without moving them to Trash: a Postponed Draft sent, or kept again.
    Expunge {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
    },
    /// Move Messages to Trash, or expunge them from Trash.
    Delete {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
    },
    /// Archive Messages: to the Archive Mailbox, or on Gmail out of the Inbox.
    Archive {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
    },
    /// Add a Message to the Mailbox with a Role, as a Draft or a copy of what was sent.
    Append {
        /// The Role of the Mailbox to add it to.
        role: Role,
        /// The Message, as RFC 5322 writes it.
        message: Vec<u8>,
        /// Whether it's a Draft, to be flagged Draft as well as Seen.
        draft: bool,
    },
    /// Send a Message through the Account's SMTP server, then keep a copy in Sent (unless the server does) and
    /// flag what it answers as answered.
    Send {
        /// The Message, made from its Draft.
        built: Box<Built>,
        /// The Message it answers, by its Mailbox and UID.
        answers: Option<(String, u32)>,
    },
}

/// A Flag the UI sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// Read; a Message without it is Unread.
    Seen,
    /// Marked as important by the user; Gmail calls it starred.
    Flagged,
    /// Replied to.
    Answered,
}

impl Flag {
    fn imap(self) -> &'static str {
        match self {
            Self::Seen => "\\Seen",
            Self::Flagged => "\\Flagged",
            Self::Answered => "\\Answered",
        }
    }
}

/// What an Account tells the UI.
#[derive(Clone, Debug)]
pub enum Update {
    /// Where the connection stands.
    Connection(Connection),
    /// A Message was sent, or couldn't be, for this reason.
    Sent(Result<(), String>),
    /// Its Mailboxes, as the server lists them.
    Mailboxes(Vec<Mailbox>),
    /// A Mailbox's Messages, counted.
    Counts {
        /// The Mailbox, by its name on the server.
        mailbox: String,
        /// How many Messages it has, and how many are Unread.
        counts: Counts,
    },
    /// The Envelopes of a Mailbox's Messages that muttrs has, oldest first, of `exists` in it.
    Envelopes {
        /// The Mailbox, by its name on the server.
        mailbox: String,
        /// The Envelopes, oldest first.
        envelopes: Vec<Envelope>,
        /// How many Messages the Mailbox has, listed or not.
        exists: u32,
    },
    /// The whole of a Message, or why it couldn't be had.
    Body {
        /// The Mailbox it's in, by its name on the server.
        mailbox: String,
        /// Its UID there.
        uid: u32,
        /// Its bytes, as RFC 5322 writes it, or why it couldn't be had.
        body: Result<Arc<Vec<u8>>, String>,
    },
    /// What a search found: the Envelopes of the newest Messages, oldest first, of how many it found.
    Found {
        /// The Mailbox searched.
        mailbox: String,
        /// What was searched for.
        query: String,
        /// The newest Messages found.
        envelopes: Vec<Envelope>,
        /// How many it found in all.
        count: usize,
    },
    /// Something asked for is done, as this says.
    Done(String),
    /// Something asked for failed, as this says.
    Failed(String),
}

/// Where an Account's connection stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    /// Connecting and signing in, the first time or again.
    Connecting,
    /// Connected and signed in.
    Online,
    /// Not connected, for this reason; muttrs tries again.
    Offline(String),
    /// The server refused to sign in, for this reason; muttrs waits for [`Request::Refresh`].
    SignInFailed(String),
}

/// What an Account's client tells its updates to.
pub type Notify = Arc<dyn Fn(Update) + Send + Sync>;

/// The handle the UI keeps on an Account.
#[derive(Clone, Debug)]
pub struct Client {
    requests: mpsc::UnboundedSender<Request>,
}

impl Client {
    /// Starts `account`'s client, keeping bodies in `cache` and telling `notify` what happens.
    ///
    /// # Errors
    ///
    /// If TLS can't be set up for it.
    pub fn spawn(account: Account, cache: Cache, notify: Notify) -> Result<Self, tls::Error> {
        let tls = tls::client_config(&account.trust)?;
        let account = Arc::new(account);
        let credentials = Arc::new(CredentialSource::new(Arc::clone(&account)));
        let (requests, received) = mpsc::unbounded_channel();
        let (inbox_changed, changes) = mpsc::unbounded_channel();
        let worker = Worker {
            own: requests.clone(),
            account: Arc::clone(&account),
            tls: Arc::clone(&tls),
            credentials: Arc::clone(&credentials),
            cache,
            notify,
            mailboxes: Vec::new(),
            windows: HashMap::new(),
            open: None,
            selected: None,
        };
        tokio::spawn(worker.run(received, changes));
        tokio::spawn(watch(account, tls, credentials, inbox_changed));
        Ok(Self { requests })
    }

    /// Asks the Account for something; what comes of it comes as [`Update`]s.
    pub fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

/// Where the credentials come from, and the access token while it's good.
struct CredentialSource {
    account: Arc<Account>,
    token: Mutex<Option<(String, Instant)>>,
}

impl CredentialSource {
    fn new(account: Arc<Account>) -> Self {
        Self {
            account,
            token: Mutex::new(None),
        }
    }

    async fn credentials(&self) -> Result<Credentials, String> {
        let account = &self.account;
        let username = account.username.clone();
        match &account.auth {
            Auth::Password(source) => {
                let password = match source {
                    PasswordSource::Keyring => {
                        secret::read(&account.name, secret::Kind::Password).await
                    }
                    PasswordSource::Command(command) => secret::from_command(command).await,
                }
                .map_err(|error| error.to_string())?;
                Ok(Credentials::Password { username, password })
            }
            Auth::OAuth2(config) => {
                let mut token = self.token.lock().await;
                if let Some((access, expires)) = token.as_ref()
                    && *expires > Instant::now() + Duration::from_secs(60)
                {
                    return Ok(Credentials::Bearer {
                        username,
                        token: access.clone(),
                    });
                }
                let refresh_token = secret::read(&account.name, secret::Kind::RefreshToken)
                    .await
                    .map_err(|error| error.to_string())?;
                let client_secret = client_secret(config.client_secret.as_ref())
                    .await
                    .map_err(|error| error.to_string())?;
                let tokens = oauth2::refresh(config, client_secret.as_deref(), &refresh_token)
                    .await
                    .map_err(|error| format!("refreshing the OAuth2 token: {error}"))?;
                // Microsoft rotates refresh tokens: the new one replaces the old.
                if let Some(rotated) = tokens
                    .refresh_token
                    .as_deref()
                    .filter(|new| *new != refresh_token)
                    && let Err(error) =
                        secret::store(&account.name, secret::Kind::RefreshToken, rotated).await
                {
                    tracing::warn!(%error, "a new refresh token couldn't be kept");
                }
                *token = Some((tokens.access_token.clone(), tokens.expires));
                Ok(Credentials::Bearer {
                    username,
                    token: tokens.access_token,
                })
            }
        }
    }

    /// Forgets the access token, which the server refused.
    async fn forget_token(&self) {
        *self.token.lock().await = None;
    }
}

/// The OAuth2 client's secret, if it has one.
///
/// # Errors
///
/// If it's printed by a command that fails.
pub async fn client_secret(secret: Option<&Secret>) -> Result<Option<String>, secret::Error> {
    match secret {
        None => Ok(None),
        Some(Secret::Value(value)) => Ok(Some(value.clone())),
        Some(Secret::Command(command)) => secret::from_command(command).await.map(Some),
    }
}

async fn connect(
    account: &Account,
    tls: &Arc<ClientConfig>,
    credentials: &CredentialSource,
) -> Result<Session, Failure> {
    let signing_in = credentials.credentials().await.map_err(Failure::SignIn)?;
    match Session::connect(&account.imap, Arc::clone(tls), &signing_in).await {
        Ok(session) => Ok(session),
        Err(error) if error.is_sign_in() => {
            credentials.forget_token().await;
            Err(Failure::SignIn(error.to_string()))
        }
        // A certificate that isn't trusted won't be on the next try either: that waits for the user too.
        Err(error @ imap::Error::Tls(tls::Error::Untrusted { .. })) => {
            Err(Failure::SignIn(error.to_string()))
        }
        Err(error) => Err(Failure::Network(error.to_string())),
    }
}

/// Why connecting failed.
enum Failure {
    /// Signing in failed, or the credentials couldn't be had: trying again won't help until the user acts.
    SignIn(String),
    Network(String),
}

/// A Mailbox's Messages that muttrs lists.
#[derive(Debug, Default)]
struct Window {
    uid_validity: u32,
    exists: u32,
    envelopes: BTreeMap<u32, Envelope>,
}

struct Worker {
    /// What the worker asks of itself, after sending.
    own: mpsc::UnboundedSender<Request>,
    account: Arc<Account>,
    tls: Arc<ClientConfig>,
    credentials: Arc<CredentialSource>,
    cache: Cache,
    notify: Notify,
    mailboxes: Vec<Mailbox>,
    windows: HashMap<String, Window>,
    /// The Mailbox the UI has open, opened again after reconnecting.
    open: Option<String>,
    /// The Mailbox the session has selected.
    selected: Option<String>,
}

impl Worker {
    async fn run(
        mut self,
        mut requests: mpsc::UnboundedReceiver<Request>,
        mut changes: mpsc::UnboundedReceiver<()>,
    ) {
        let mut backoff = Duration::from_secs(2);
        loop {
            (self.notify)(Update::Connection(Connection::Connecting));
            let session = match connect(&self.account, &self.tls, &self.credentials).await {
                Ok(session) => session,
                Err(Failure::SignIn(why)) => {
                    tracing::warn!(account = self.account.name, why, "signing in failed");
                    (self.notify)(Update::Connection(Connection::SignInFailed(why)));
                    if !self.offline(&mut requests, None).await {
                        return;
                    }
                    continue;
                }
                Err(Failure::Network(why)) => {
                    tracing::info!(account = self.account.name, why, "offline");
                    (self.notify)(Update::Connection(Connection::Offline(why)));
                    if !self.offline(&mut requests, Some(backoff)).await {
                        return;
                    }
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };
            backoff = Duration::from_secs(2);
            (self.notify)(Update::Connection(Connection::Online));
            self.selected = None;
            match self.serve(session, &mut requests, &mut changes).await {
                Ok(()) => return,
                Err(error) => {
                    tracing::info!(account = self.account.name, %error, "the connection dropped");
                    (self.notify)(Update::Connection(Connection::Offline(error.to_string())));
                }
            }
        }
    }

    /// Waits for [`Request::Refresh`], or `timeout` when there is one, doing meanwhile what the cache can: opening a
    /// Mailbox it keeps the Envelopes of, reading a Message it keeps. `false` if nothing more can be asked.
    async fn offline(
        &mut self,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        timeout: Option<Duration>,
    ) -> bool {
        let waiting = async {
            loop {
                match requests.recv().await {
                    Some(Request::Refresh) => return true,
                    Some(Request::Open(mailbox)) => {
                        self.open = Some(mailbox.clone());
                        self.listed_from_cache(&mailbox);
                        self.tell_window(&mailbox);
                    }
                    Some(Request::Body { mailbox, uid }) => {
                        let validity = self.windows.get(&mailbox).map(|window| window.uid_validity);
                        let body = match validity {
                            Some(validity) => self.cache.body(&mailbox, validity, uid).await,
                            None => None,
                        };
                        (self.notify)(Update::Body {
                            mailbox,
                            uid,
                            body: body
                                .map(Arc::new)
                                .ok_or_else(|| "offline, and it isn't cached".to_owned()),
                        });
                    }
                    Some(Request::Send { .. }) => {
                        (self.notify)(Update::Sent(Err(
                            "offline: the Draft stays, to send once online".into(),
                        )));
                    }
                    // What changes the server waits for it.
                    Some(_) => {
                        (self.notify)(Update::Failed("Offline: that waits for the server".into()));
                    }
                    None => return false,
                }
            }
        };
        match timeout {
            Some(timeout) => tokio::time::timeout(timeout, waiting).await.unwrap_or(true),
            None => waiting.await,
        }
    }

    /// Does what's asked over `session` until it fails, or nothing more can be asked.
    async fn serve(
        &mut self,
        mut session: Session,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        changes: &mut mpsc::UnboundedReceiver<()>,
    ) -> Result<(), imap::Error> {
        self.list(&mut session).await?;
        let mut counts_due = self.counts_due();
        if let Some(open) = self.open.clone() {
            let opened = self.open(&mut session, &open).await;
            self.tolerate(opened)?;
        }
        loop {
            // What's asked comes first; counting Mailboxes' Messages fills the gaps between.
            let request = if let Some(mailbox) = counts_due.front().cloned() {
                match requests.try_recv() {
                    Ok(request) => Some(request),
                    Err(mpsc::error::TryRecvError::Empty) => {
                        counts_due.pop_front();
                        let counted = self.count(&mut session, &mailbox).await;
                        self.tolerate(counted)?;
                        continue;
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => None,
                }
            } else {
                tokio::select! {
                    request = requests.recv() => request,
                    Some(()) = changes.recv() => {
                        let seen = self.changed(&mut session).await;
                        self.tolerate(seen)?;
                        continue;
                    }
                    () = tokio::time::sleep(POLL) => {
                        // Keeps the connection alive, and sees what's new in the open Mailbox.
                        if let Some(open) = self.open.clone() {
                            let opened = self.open(&mut session, &open).await;
                            self.tolerate(opened)?;
                        }
                        continue;
                    }
                }
            };
            let Some(request) = request else {
                session.logout().await;
                return Ok(());
            };
            let handled = self.handle(&mut session, request, &mut counts_due).await;
            self.tolerate(handled)?;
        }
    }

    /// Does `request` over `session`.
    async fn handle(
        &mut self,
        session: &mut Session,
        request: Request,
        counts_due: &mut VecDeque<String>,
    ) -> Result<(), imap::Error> {
        match request {
            Request::Refresh => {
                self.list(session).await?;
                *counts_due = self.counts_due();
                if let Some(open) = self.open.clone() {
                    self.open(session, &open).await?;
                }
            }
            Request::Open(mailbox) => {
                self.open = Some(mailbox.clone());
                self.open(session, &mailbox).await?;
            }
            Request::More(mailbox) => self.more(session, &mailbox).await?,
            Request::Search { mailbox, query } => self.search(session, mailbox, query).await?,
            Request::Body { mailbox, uid } => self.body(session, &mailbox, uid).await?,
            Request::Flag {
                mailbox,
                uids,
                flag,
                on,
            } => {
                self.select(session, &mailbox).await?;
                if let Err(error) = session.set_flag(&uids, flag.imap(), on).await {
                    (self.notify)(Update::Failed(format!("Changing flags: {error}")));
                }
                self.refresh_window(session, &mailbox).await?;
            }
            Request::MoveTo {
                mailbox,
                uids,
                role,
            } => {
                self.move_to(session, &mailbox, &uids, role).await?;
            }
            Request::Move { mailbox, uids, to } => {
                self.move_named(session, &mailbox, &uids, &to).await?;
            }
            Request::Expunge { mailbox, uids } => {
                self.select(session, &mailbox).await?;
                if let Err(error) = session.expunge(&uids).await {
                    (self.notify)(Update::Failed(format!("Expunging: {error}")));
                }
                self.refresh_window(session, &mailbox).await?;
                self.count(session, &mailbox).await?;
            }
            Request::Delete { mailbox, uids } => {
                self.delete(session, &mailbox, &uids).await?;
            }
            Request::Archive { mailbox, uids } => {
                let role = if self.account.gmail {
                    Role::All
                } else {
                    Role::Archive
                };
                self.move_to(session, &mailbox, &uids, role).await?;
            }
            Request::Send { built, answers } => self.send(*built, answers),
            Request::Append {
                role,
                message,
                draft,
            } => {
                let Some(target) = self.with_role(role) else {
                    (self.notify)(Update::Failed(format!("There's no {role:?} Mailbox")));
                    return Ok(());
                };
                let flags = if draft {
                    "(\\Seen \\Draft)"
                } else {
                    "(\\Seen)"
                };
                match session.append(&target, flags, &message).await {
                    Ok(()) => self.count(session, &target).await?,
                    Err(error) => {
                        (self.notify)(Update::Failed(format!("Saving to {target}: {error}")));
                    }
                }
            }
        }
        Ok(())
    }

    /// Sends `built` in the background, so that reading goes on meanwhile; then asks for the copy in Sent and
    /// the answered flag.
    fn send(&self, built: Built, answers: Option<(String, u32)>) {
        let (account, credentials, notify, own) = (
            Arc::clone(&self.account),
            Arc::clone(&self.credentials),
            Arc::clone(&self.notify),
            self.own.clone(),
        );
        tokio::spawn(async move {
            let sent = async {
                let signing_in = credentials.credentials().await?;
                smtp::send(&account.smtp, &account.trust, &signing_in, &built)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            if sent.is_ok() {
                if account.copy_to_sent {
                    let _ = own.send(Request::Append {
                        role: Role::Sent,
                        message: built.kept,
                        draft: false,
                    });
                }
                if let Some((mailbox, uid)) = answers {
                    let _ = own.send(Request::Flag {
                        mailbox,
                        uids: vec![uid],
                        flag: Flag::Answered,
                        on: true,
                    });
                }
            }
            notify(Update::Sent(sent));
        });
    }

    /// `result`, unless it failed in a way that leaves the connection fine (the server refused a command, say),
    /// which is told and done with.
    fn tolerate(&self, result: Result<(), imap::Error>) -> Result<(), imap::Error> {
        match result {
            Err(error) if !error.is_fatal() => {
                tracing::info!(account = self.account.name, %error, "a command failed");
                (self.notify)(Update::Failed(error.to_string()));
                Ok(())
            }
            other => other,
        }
    }

    async fn list(&mut self, session: &mut Session) -> Result<(), imap::Error> {
        self.mailboxes = session.mailboxes().await?;
        self.cache.keep_mailboxes(&self.mailboxes);
        (self.notify)(Update::Mailboxes(self.mailboxes.clone()));
        Ok(())
    }

    /// The Mailboxes to count the Messages of, the Inbox first.
    fn counts_due(&self) -> VecDeque<String> {
        let mut due: Vec<&Mailbox> = self
            .mailboxes
            .iter()
            .filter(|mailbox| mailbox.selectable)
            .collect();
        due.sort_by_key(|mailbox| mailbox.role != Some(Role::Inbox));
        due.into_iter()
            .map(|mailbox| mailbox.name.clone())
            .collect()
    }

    async fn count(&mut self, session: &mut Session, mailbox: &str) -> Result<(), imap::Error> {
        match session.counts(mailbox).await {
            Ok(counts) => (self.notify)(Update::Counts {
                mailbox: mailbox.to_owned(),
                counts,
            }),
            // A Mailbox that can't be counted (gone since it was listed) isn't the connection failing.
            Err(imap::Error::Protocol(async_imap::error::Error::No(_))) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    /// The name of the Mailbox with `role`: the configuration's, or the server's.
    fn with_role(&self, role: Role) -> Option<String> {
        let configured = match role {
            Role::Sent => self.account.roles.sent.clone(),
            Role::Drafts => self.account.roles.drafts.clone(),
            Role::Trash => self.account.roles.trash.clone(),
            Role::Junk => self.account.roles.junk.clone(),
            Role::Archive => self.account.roles.archive.clone(),
            _ => None,
        };
        configured
            .map(|name| crate::mailbox::encode(&name))
            .or_else(|| {
                self.mailboxes
                    .iter()
                    .find(|mailbox| mailbox.role == Some(role))
                    .map(|mailbox| mailbox.name.clone())
            })
    }

    async fn select(
        &mut self,
        session: &mut Session,
        mailbox: &str,
    ) -> Result<imap::Selected, imap::Error> {
        let selected = session.select(mailbox).await?;
        self.selected = Some(mailbox.to_owned());
        Ok(selected)
    }

    /// Opens `mailbox`, and tells its Envelopes: the newest, the first time; what's new and changed since, after.
    async fn open(&mut self, session: &mut Session, mailbox: &str) -> Result<(), imap::Error> {
        // What the cache keeps is brought up to date, rather than fetched again.
        self.listed_from_cache(mailbox);
        let selected = self.select(session, mailbox).await?;
        let window = self.windows.entry(mailbox.to_owned()).or_default();
        if window.uid_validity != selected.uid_validity || window.envelopes.is_empty() {
            *window = Window {
                uid_validity: selected.uid_validity,
                exists: selected.exists,
                envelopes: session
                    .newest(selected.exists, WINDOW)
                    .await?
                    .into_iter()
                    .map(|envelope| (envelope.uid, envelope))
                    .collect(),
            };
        } else {
            window.exists = selected.exists;
            Self::update_window(session, window).await?;
        }
        self.tell_window(mailbox);
        let unread = session.unread().await?;
        if let Some(window) = self.windows.get(mailbox) {
            (self.notify)(Update::Counts {
                mailbox: mailbox.to_owned(),
                counts: Counts {
                    messages: window.exists,
                    unread,
                },
            });
        }
        Ok(())
    }

    /// Searches `mailbox` for `query`, and tells the Envelopes of the newest Messages found.
    async fn search(
        &mut self,
        session: &mut Session,
        mailbox: String,
        query: String,
    ) -> Result<(), imap::Error> {
        if self.selected.as_deref() != Some(mailbox.as_str()) {
            self.select(session, &mailbox).await?;
        }
        let uids = session.search(&query, self.account.gmail).await?;
        let newest = &uids[uids.len().saturating_sub(WINDOW as usize)..];
        let envelopes = if newest.is_empty() {
            Vec::new()
        } else {
            session.envelopes(&imap::uid_set(newest)).await?
        };
        (self.notify)(Update::Found {
            mailbox,
            query,
            envelopes,
            count: uids.len(),
        });
        Ok(())
    }

    /// Lists older Envelopes of `mailbox` than those it lists, and tells them all.
    async fn more(&mut self, session: &mut Session, mailbox: &str) -> Result<(), imap::Error> {
        let Some(first) = self
            .windows
            .get(mailbox)
            .and_then(|window| window.envelopes.keys().next().copied())
        else {
            return Ok(());
        };
        if self.selected.as_deref() != Some(mailbox) {
            self.select(session, mailbox).await?;
        }
        // The oldest listed Message's sequence number says where the older ones end.
        if let Some(sequence) = session.sequence_of(first).await?
            && sequence > 1
        {
            let older = session
                .between(sequence.saturating_sub(WINDOW).max(1), sequence - 1)
                .await?;
            if let Some(window) = self.windows.get_mut(mailbox) {
                window
                    .envelopes
                    .extend(older.into_iter().map(|envelope| (envelope.uid, envelope)));
            }
        }
        self.tell_window(mailbox);
        Ok(())
    }

    /// The Envelopes of what's new in `window`'s Mailbox, which is selected, and the Flags of what it has.
    async fn update_window(session: &mut Session, window: &mut Window) -> Result<(), imap::Error> {
        let Some((&first, _)) = window.envelopes.first_key_value() else {
            return Ok(());
        };
        let last = window
            .envelopes
            .last_key_value()
            .map_or(first, |(&uid, _)| uid);
        // `last+1:*` names the last Message even when there's nothing after `last`: hence the filter.
        for envelope in session.envelopes(&format!("{}:*", last + 1)).await? {
            if envelope.uid > last {
                window.envelopes.insert(envelope.uid, envelope);
            }
        }
        let flags: HashMap<u32, Flags> = session
            .flags(&format!("{first}:{last}"))
            .await?
            .into_iter()
            .collect();
        // What's no longer there was expunged.
        window
            .envelopes
            .retain(|uid, _| *uid > last || flags.contains_key(uid));
        for (uid, envelope) in &mut window.envelopes {
            if let Some(flags) = flags.get(uid) {
                envelope.flags = *flags;
            }
        }
        // Those listed before Gmail's Threads were asked for, as the cache may keep them.
        session
            .add_gmail_threads(window.envelopes.values_mut())
            .await?;
        Ok(())
    }

    async fn refresh_window(
        &mut self,
        session: &mut Session,
        mailbox: &str,
    ) -> Result<(), imap::Error> {
        if let Some(window) = self.windows.get_mut(mailbox) {
            Self::update_window(session, window).await?;
            self.tell_window(mailbox);
        }
        Ok(())
    }

    /// Tells the UI the Envelopes of `mailbox` listed, keeping them in the cache, so that they show at once the next
    /// time, and offline.
    fn tell_window(&self, mailbox: &str) {
        if let Some(window) = self.windows.get(mailbox) {
            let listed = Listed {
                uid_validity: window.uid_validity,
                exists: window.exists,
                envelopes: window.envelopes.values().cloned().collect(),
            };
            self.cache.keep_envelopes(mailbox, &listed);
            (self.notify)(Update::Envelopes {
                mailbox: mailbox.to_owned(),
                envelopes: listed.envelopes,
                exists: listed.exists,
            });
        }
    }

    /// The Envelopes of `mailbox` kept in the cache, listed again, unless some are listed already.
    fn listed_from_cache(&mut self, mailbox: &str) {
        if self.windows.contains_key(mailbox) {
            return;
        }
        if let Some(listed) = self.cache.envelopes(mailbox) {
            self.windows.insert(
                mailbox.to_owned(),
                Window {
                    uid_validity: listed.uid_validity,
                    exists: listed.exists,
                    envelopes: listed
                        .envelopes
                        .into_iter()
                        .map(|envelope| (envelope.uid, envelope))
                        .collect(),
                },
            );
        }
    }

    /// What IDLE saw: the Inbox changed.
    async fn changed(&mut self, session: &mut Session) -> Result<(), imap::Error> {
        let inbox = self
            .mailboxes
            .iter()
            .find(|mailbox| mailbox.role == Some(Role::Inbox))
            .map_or_else(|| "INBOX".to_owned(), |mailbox| mailbox.name.clone());
        if self.open.as_deref() == Some(inbox.as_str()) {
            self.open(session, &inbox).await
        } else {
            self.count(session, &inbox).await
        }
    }

    async fn body(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        uid: u32,
    ) -> Result<(), imap::Error> {
        let uid_validity = self.windows.get(mailbox).map(|window| window.uid_validity);
        if let Some(validity) = uid_validity
            && let Some(body) = self.cache.body(mailbox, validity, uid).await
        {
            (self.notify)(Update::Body {
                mailbox: mailbox.to_owned(),
                uid,
                body: Ok(Arc::new(body)),
            });
            return Ok(());
        }
        let selected = if self.selected.as_deref() == Some(mailbox) {
            None
        } else {
            Some(self.select(session, mailbox).await?)
        };
        let validity = uid_validity.or(selected.map(|selected| selected.uid_validity));
        let body = match session.body(uid).await {
            Ok(body) => {
                if let Some(validity) = validity {
                    self.cache.keep(mailbox, validity, uid, &body).await;
                }
                Ok(Arc::new(body))
            }
            Err(imap::Error::Missing(what)) => Err(format!("The server sent no {what}")),
            Err(error) => return Err(error),
        };
        (self.notify)(Update::Body {
            mailbox: mailbox.to_owned(),
            uid,
            body,
        });
        Ok(())
    }

    async fn move_to(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        uids: &[u32],
        role: Role,
    ) -> Result<(), imap::Error> {
        let Some(target) = self.with_role(role) else {
            (self.notify)(Update::Failed(format!(
                "There's no {role:?} Mailbox: set it under [account.mailboxes]"
            )));
            // What the UI took out before asking comes back.
            self.tell_window(mailbox);
            return Ok(());
        };
        self.move_named(session, mailbox, uids, &target).await
    }

    /// Moves the Messages `uids` of `mailbox` to the Mailbox named `target` on the server.
    async fn move_named(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        uids: &[u32],
        target: &str,
    ) -> Result<(), imap::Error> {
        if target == mailbox {
            self.tell_window(mailbox);
            return Ok(());
        }
        self.select(session, mailbox).await?;
        match session.move_to(uids, target).await {
            Ok(()) => {
                let label = crate::mailbox::decode(target);
                (self.notify)(Update::Done(format!(
                    "Moved {} to {label}",
                    count(uids.len())
                )));
            }
            Err(error) => (self.notify)(Update::Failed(format!("Moving: {error}"))),
        }
        self.refresh_window(session, mailbox).await?;
        self.count(session, target).await
    }

    async fn delete(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        uids: &[u32],
    ) -> Result<(), imap::Error> {
        if self.with_role(Role::Trash).as_deref() != Some(mailbox) {
            return self.move_to(session, mailbox, uids, Role::Trash).await;
        }
        self.select(session, mailbox).await?;
        match session.expunge(uids).await {
            Ok(()) => (self.notify)(Update::Done(format!(
                "Deleted {} for good",
                count(uids.len())
            ))),
            Err(error) => (self.notify)(Update::Failed(format!("Deleting: {error}"))),
        }
        self.refresh_window(session, mailbox).await
    }
}

/// "a Message", or "3 Messages".
fn count(messages: usize) -> String {
    if messages == 1 {
        "a Message".into()
    } else {
        format!("{messages} Messages")
    }
}

/// Watches the Inbox from a session of its own, telling `changed` when there's something new: by IDLE, or by
/// asking every few minutes where the server has no IDLE.
async fn watch(
    account: Arc<Account>,
    tls: Arc<ClientConfig>,
    credentials: Arc<CredentialSource>,
    changed: mpsc::UnboundedSender<()>,
) {
    let mut backoff = Duration::from_secs(5);
    loop {
        if changed.is_closed() {
            return;
        }
        let mut session = match connect(&account, &tls, &credentials).await {
            Ok(session) => session,
            Err(Failure::SignIn(_) | Failure::Network(_)) => {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = Duration::from_secs(5);
        if session.select("INBOX").await.is_err() {
            tokio::time::sleep(MAX_BACKOFF).await;
            continue;
        }
        loop {
            if !session.can_idle() {
                tokio::time::sleep(POLL).await;
                if changed.send(()).is_err() {
                    return;
                }
                continue;
            }
            match session.idle(IDLE_RENEWAL).await {
                Ok((renewed, new)) => {
                    session = renewed;
                    if new && changed.send(()).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    tracing::debug!(account = account.name, %error, "IDLE stopped");
                    break;
                }
            }
        }
    }
}
