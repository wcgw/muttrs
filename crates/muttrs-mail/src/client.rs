//! An Account's client: a worker task that owns the IMAP session and does what the UI asks, reconnecting when the
//! connection drops, and a watcher that IDLEs on the Inbox for new mail. The UI never waits on the network: it
//! sends [`Request`]s and is told [`Update`]s.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rustls::ClientConfig;
use tokio::sync::{Mutex, mpsc};

use crate::account::{Account, Auth, PasswordSource, Secret};
use crate::cache::{Cache, Listed};
use crate::compose::Built;
use crate::envelope::{Envelope, Flags};
use crate::imap::{self, Credentials, Session};
use crate::label::{self, Label, Members};
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
/// How long moving Messages back waits, each time they aren't found where they went, before looking again: a server
/// can take a while to show them there (Proton's Bridge, archiving). After the last, it gives up.
const MOVE_BACK_WAITS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];

/// A Request asked of an Account, numbered by its Client: what [`Update::Started`] and [`Update::Finished`] tell of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Task(u64);

/// What the UI asks of an Account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// List its Mailboxes again, and connect again now if it's offline.
    Refresh,
    /// Forget what's kept of a Mailbox, its Envelopes listed (the cache's too) and which Messages each Label has, and
    /// list it anew, as when it was first opened; and the Mailboxes, as [`Request::Refresh`] does.
    Resync(String),
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
    /// Search every Mailbox on the server, for how many Messages each has that it finds.
    SearchAll {
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
    /// Add a Label to Messages, or take it off them.
    Label {
        /// The Mailbox they're in, by its name on the server.
        mailbox: String,
        /// Their UIDs there.
        uids: Vec<u32>,
        /// The Label, by its Mailbox's name on the server.
        label: String,
        /// Whether to add it, rather than take it off.
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
    /// Move Messages back to where they were moved from, as undoing it does: those with `message_ids` (and only
    /// those), found by them where they went.
    MoveBack {
        /// Where they went.
        went: Went,
        /// Where they were, by its name on the server.
        to: String,
        /// Their `Message-ID`s.
        message_ids: Vec<String>,
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
    /// Create a Mailbox, named for people as `Work/Reports`.
    CreateMailbox(String),
    /// Rename a Mailbox.
    RenameMailbox {
        /// The Mailbox, by its name on the server.
        mailbox: String,
        /// Its new name, for people, as `Work/Reports`.
        to: String,
    },
    /// Delete a Mailbox, and its Messages with it.
    DeleteMailbox(String),
    /// Send a Message on as it is (mutt's bounce): through the Account's SMTP server, without a copy in Sent.
    Bounce {
        /// The Message, with its `Resent-` headers.
        built: Box<Built>,
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

/// Where Messages were moved to: a Mailbox, or where deleting or archiving puts them, as the worker knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Went {
    /// A Mailbox, by its name on the server.
    Mailbox(String),
    /// Trash, as deleting moves them.
    Trash,
    /// Where archiving moves them: the Archive Mailbox, or on Gmail All Mail.
    Archived,
    /// Junk, as taking them for spam moves them.
    Junk,
}

impl Request {
    /// The Mailbox whose Messages it changes, if it does: their Flags or Labels, or which Messages it has.
    fn changes(&self) -> Option<&str> {
        match self {
            Self::Flag { mailbox, .. }
            | Self::Label { mailbox, .. }
            | Self::MoveTo { mailbox, .. }
            | Self::Move { mailbox, .. }
            | Self::Expunge { mailbox, .. }
            | Self::Delete { mailbox, .. }
            | Self::Archive { mailbox, .. } => Some(mailbox),
            Self::Refresh
            | Self::Resync(_)
            | Self::Open(_)
            | Self::More(_)
            | Self::Search { .. }
            | Self::SearchAll { .. }
            | Self::Body { .. }
            | Self::MoveBack { .. }
            | Self::CreateMailbox(_)
            | Self::RenameMailbox { .. }
            | Self::DeleteMailbox(_)
            | Self::Bounce { .. }
            | Self::Append { .. }
            | Self::Send { .. } => None,
        }
    }
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
    /// The worker started on a Task: Tasks are done one at a time, in the order asked, but for a Message to read,
    /// which goes first.
    Started(Task),
    /// The worker is done with a Task, whether it did what was asked or not (as [`Update::Done`] or
    /// [`Update::Failed`] says); for one that changes a Mailbox's Messages, the Envelopes told next show them as the
    /// server has them since.
    Finished(Task),
    /// The Messages a [`Request::MoveBack`] Task was for are back where they were. Finished without it, they
    /// aren't.
    MovedBack(Task),
    /// A Message was sent, or couldn't be, for this reason.
    Sent {
        /// The Task that sent it.
        task: Task,
        /// Whether it was sent, or why not.
        sent: Result<(), String>,
    },
    /// Its Mailboxes, as the server lists them.
    Mailboxes(Vec<Mailbox>),
    /// Its Labels, by name.
    Labels(Vec<Label>),
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
    /// What a search of every Mailbox found: the Mailboxes with Messages it found, by their names on the server, in
    /// the order they're listed, and how many each has.
    FoundIn {
        /// What was searched for.
        query: String,
        /// The Mailboxes, and how many it found in each.
        found: Vec<(String, usize)>,
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
    requests: mpsc::UnboundedSender<(Task, Request)>,
    /// The number of the next Task asked.
    next: Arc<AtomicU64>,
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
        let (client, received) = Self::detached();
        let (inbox_changed, changes) = mpsc::unbounded_channel();
        let worker = Worker {
            own: client.clone(),
            account: Arc::clone(&account),
            tls: Arc::clone(&tls),
            credentials: Arc::clone(&credentials),
            cache,
            notify,
            mailboxes: Vec::new(),
            gmail: false,
            labels: Vec::new(),
            members: HashMap::new(),
            windows: HashMap::new(),
            open: None,
            selected: None,
            selected_modseq: None,
            queue: VecDeque::new(),
            again: Vec::new(),
        };
        tokio::spawn(worker.run(received, changes));
        tokio::spawn(watch(account, tls, credentials, inbox_changed));
        Ok(client)
    }

    /// A Client of no Account, for trying a UI without one: what's asked of it waits in the receiver returned.
    #[must_use]
    pub fn detached() -> (Self, mpsc::UnboundedReceiver<(Task, Request)>) {
        let (requests, received) = mpsc::unbounded_channel();
        let client = Self {
            requests,
            next: Arc::new(AtomicU64::new(1)),
        };
        (client, received)
    }

    /// Asks the Account for something, as the Task returned; what comes of it comes as [`Update`]s.
    #[allow(clippy::must_use_candidate)] // Asking is what it's for: the Task, for those that follow it.
    pub fn send(&self, request: Request) -> Task {
        let task = Task(self.next.fetch_add(1, Ordering::Relaxed));
        let _ = self.requests.send((task, request));
        task
    }
}

/// Where the credentials come from, and the access token while it's good.
struct CredentialSource {
    account: Arc<Account>,
    token: Mutex<Option<(String, SystemTime)>>,
}

impl CredentialSource {
    fn new(account: Arc<Account>) -> Self {
        Self {
            account,
            token: Mutex::new(None),
        }
    }

    async fn credentials(&self) -> Result<Credentials, Failure> {
        let account = &self.account;
        let username = account.username.clone();
        let unavailable = |error: secret::Error| Failure::SignIn(error.to_string());
        match &account.auth {
            Auth::Password(source) => {
                let password = match source {
                    PasswordSource::Keyring => {
                        secret::read(&account.name, secret::Kind::Password).await
                    }
                    PasswordSource::Command(command) => secret::from_command(command).await,
                }
                .map_err(unavailable)?;
                Ok(Credentials::Password { username, password })
            }
            Auth::OAuth2(config) => {
                let mut token = self.token.lock().await;
                if let Some((access, expires)) = token.as_ref()
                    && *expires > SystemTime::now() + Duration::from_secs(60)
                {
                    return Ok(Credentials::Bearer {
                        username,
                        token: access.clone(),
                    });
                }
                let refresh_token = secret::read(&account.name, secret::Kind::RefreshToken)
                    .await
                    .map_err(unavailable)?;
                let client_secret = client_secret(config.client_secret.as_ref())
                    .await
                    .map_err(unavailable)?;
                let tokens = oauth2::refresh(config, client_secret.as_deref(), &refresh_token)
                    .await
                    .map_err(|error| Failure::refreshing(&error))?;
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
    let signing_in = credentials.credentials().await?;
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
    /// The server, or the OAuth2 provider, couldn't be reached: trying again, later, may.
    Network(String),
}

impl Failure {
    /// Why refreshing the OAuth2 access token failed. Not reaching the provider (on waking, before the network is
    /// back, say), or getting a page that isn't its answer (a captive portal's), is tried again; the provider
    /// refusing waits for the user.
    fn refreshing(error: &oauth2::Error) -> Self {
        let why = format!("refreshing the OAuth2 token: {error}");
        match error {
            oauth2::Error::Http(_) => Self::Network(why),
            oauth2::Error::Refused(_)
            | oauth2::Error::NoRefreshToken
            | oauth2::Error::Redirect(_)
            | oauth2::Error::TimedOut
            | oauth2::Error::NotTheRedirect => Self::SignIn(why),
        }
    }

    fn why(self) -> String {
        match self {
            Self::SignIn(why) | Self::Network(why) => why,
        }
    }
}

/// A Mailbox's Messages that muttrs lists.
#[derive(Debug, Default)]
struct Window {
    uid_validity: u32,
    exists: u32,
    envelopes: BTreeMap<u32, Envelope>,
    /// The Mailbox's highest mod-sequence (CONDSTORE's) when they were brought up to date: what's changed since is
    /// what's asked, rather than every one's Flags.
    highest_modseq: Option<u64>,
}

struct Worker {
    /// What the worker asks of itself, after sending.
    own: Client,
    account: Arc<Account>,
    tls: Arc<ClientConfig>,
    credentials: Arc<CredentialSource>,
    cache: Cache,
    notify: Notify,
    mailboxes: Vec<Mailbox>,
    /// Whether the server tells Gmail's labels (`X-GM-LABELS`), rather than keep Labels as Mailboxes of their own.
    gmail: bool,
    labels: Vec<Label>,
    /// Which Messages each Label's Mailbox has, by its name, where the server doesn't tell Gmail's labels.
    members: HashMap<String, Members>,
    windows: HashMap<String, Window>,
    /// The Mailbox the UI has open, opened again after reconnecting.
    open: Option<String>,
    /// The Mailbox the session has selected.
    selected: Option<String>,
    /// Its highest mod-sequence when it was, as the server said.
    selected_modseq: Option<u64>,
    /// What's asked and not started yet, in the order asked: kept while offline, and across connections.
    queue: VecDeque<(Task, Request)>,
    /// What's tried again once it's due, as moving back Messages not found where they went yet: when, the Task,
    /// and how many times it was tried.
    again: Vec<(tokio::time::Instant, Task, Request, usize)>,
}

impl Worker {
    async fn run(
        mut self,
        mut requests: mpsc::UnboundedReceiver<(Task, Request)>,
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
        requests: &mut mpsc::UnboundedReceiver<(Task, Request)>,
        timeout: Option<Duration>,
    ) -> bool {
        let until = timeout.map(|timeout| tokio::time::Instant::now() + timeout);
        loop {
            // Only waiting for what's asked times out, never doing it: each Task asked is told finished.
            let asked = match self.queue.pop_front() {
                Some(asked) => Some(asked),
                None => match until {
                    Some(until) => match tokio::time::timeout_at(until, requests.recv()).await {
                        Ok(asked) => asked,
                        Err(_) => return true,
                    },
                    None => requests.recv().await,
                },
            };
            let Some((task, request)) = asked else {
                return false;
            };
            (self.notify)(Update::Started(task));
            let changes = request.changes().map(str::to_owned);
            let refresh = request == Request::Refresh;
            match request {
                Request::Refresh => {}
                Request::Open(mailbox) => {
                    self.open = Some(mailbox.clone());
                    self.listed_from_cache(&mailbox);
                    self.tell_window(&mailbox);
                }
                Request::Body { mailbox, uid } => {
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
                Request::Send { .. } => {
                    (self.notify)(Update::Sent {
                        task,
                        sent: Err("offline: send it again once online".into()),
                    });
                }
                // What changes the server waits for it.
                _ => {
                    (self.notify)(Update::Failed("Offline: that waits for the server".into()));
                }
            }
            (self.notify)(Update::Finished(task));
            // What the UI showed changed comes back as it was.
            if let Some(mailbox) = changes {
                self.tell_window_again(&mailbox);
            }
            if refresh {
                return true;
            }
        }
    }

    /// Does what's asked over `session` until it fails, or nothing more can be asked.
    async fn serve(
        &mut self,
        mut session: Session,
        requests: &mut mpsc::UnboundedReceiver<(Task, Request)>,
        changes: &mut mpsc::UnboundedReceiver<()>,
    ) -> Result<(), imap::Error> {
        self.list(&mut session).await?;
        let mut counts_due = self.counts_due();
        if let Some(open) = self.open.clone() {
            let opened = self.open(&mut session, &open).await;
            self.tolerate(opened)?;
        }
        loop {
            // What's asked comes first, and what's tried again once it's due; counting Mailboxes' Messages fills the
            // gaps between.
            if let Some((task, request, tries)) = self.due_again() {
                self.move_back_task(&mut session, task, request, tries)
                    .await?;
                continue;
            }
            let (task, request) = if let Some(asked) = self.next_asked(requests) {
                asked
            } else if let Some(mailbox) = counts_due.pop_front() {
                let counted = self.count(&mut session, &mailbox).await;
                self.tolerate(counted)?;
                continue;
            } else {
                tokio::select! {
                    asked = requests.recv() => {
                        let Some(asked) = asked else {
                            session.logout().await;
                            return Ok(());
                        };
                        asked
                    }
                    Some(()) = changes.recv() => {
                        let seen = self.changed(&mut session).await;
                        self.tolerate(seen)?;
                        continue;
                    }
                    () = Self::until_again(self.again.iter().map(|(due, ..)| *due).min()) => continue,
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
            self.work(&mut session, task, request, &mut counts_due)
                .await?;
        }
    }

    /// Takes what's been asked since into the queue, and the next Task to do from it.
    fn next_asked(
        &mut self,
        requests: &mut mpsc::UnboundedReceiver<(Task, Request)>,
    ) -> Option<(Task, Request)> {
        while let Ok(asked) = requests.try_recv() {
            self.queue.push_back(asked);
        }
        next(&mut self.queue)
    }

    /// Does `task` over `session`, telling the UI when it starts and once it's done (a Message sent, once it's
    /// sent); then the Envelopes of the Mailbox it changed, as they stand.
    async fn work(
        &mut self,
        session: &mut Session,
        task: Task,
        request: Request,
        counts_due: &mut VecDeque<String>,
    ) -> Result<(), imap::Error> {
        if matches!(request, Request::MoveBack { .. }) {
            return self.move_back_task(session, task, request, 0).await;
        }
        (self.notify)(Update::Started(task));
        let sending = matches!(request, Request::Send { .. } | Request::Bounce { .. });
        let changes = request.changes().map(str::to_owned);
        let handled = self.handle(session, task, request, counts_due).await;
        if !sending {
            (self.notify)(Update::Finished(task));
        }
        // Told again even if it changed nothing, the server refusing, so that the UI shows them as they are.
        if let Some(mailbox) = changes {
            self.tell_window_again(&mailbox);
        }
        self.tolerate(handled)
    }

    /// Does `request`, Task `task`, over `session`.
    async fn handle(
        &mut self,
        session: &mut Session,
        task: Task,
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
            Request::Resync(mailbox) => {
                self.list(session).await?;
                *counts_due = self.counts_due();
                self.members.clear();
                self.open = Some(mailbox.clone());
                self.open_as(session, &mailbox, true).await?;
                (self.notify)(Update::Done(format!(
                    "Listed {} anew",
                    self.named(&mailbox)
                )));
            }
            Request::More(mailbox) => self.more(session, &mailbox).await?,
            Request::Search { mailbox, query } => self.search(session, mailbox, query).await?,
            Request::SearchAll { query } => self.search_all(session, query).await?,
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
            Request::Label {
                mailbox,
                uids,
                label,
                on,
            } => self.label(session, &mailbox, &uids, &label, on).await?,
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
                self.move_to(session, &mailbox, &uids, self.archive_role())
                    .await?;
            }
            // Done by `move_back_task`, which tries it again.
            Request::MoveBack { .. } => {}
            Request::Send { built, answers } => self.send(task, *built, answers, true),
            Request::Bounce { built } => self.send(task, *built, None, false),
            Request::CreateMailbox(path) => {
                let name = session.mailbox_named(&path);
                let done = session.create(&name).await;
                self.changed_mailboxes(session, done, format!("Created {path}"))
                    .await?;
            }
            Request::RenameMailbox { mailbox, to } => {
                let name = session.mailbox_named(&to);
                let from = self.named(&mailbox);
                let done = session.rename(&mailbox, &name).await;
                self.changed_mailboxes(session, done, format!("Renamed {from} {to}"))
                    .await?;
            }
            Request::DeleteMailbox(mailbox) => {
                let from = self.named(&mailbox);
                if self.selected.as_deref() == Some(mailbox.as_str()) {
                    self.selected = None;
                }
                let done = session.delete(&mailbox).await;
                self.changed_mailboxes(session, done, format!("Deleted {from}"))
                    .await?;
            }
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

    /// Tells `done`, a Mailbox created, renamed or deleted, as `said`, or why it wasn't; and lists the Mailboxes
    /// again.
    async fn changed_mailboxes(
        &mut self,
        session: &mut Session,
        done: Result<(), imap::Error>,
        said: String,
    ) -> Result<(), imap::Error> {
        match done {
            Ok(()) => (self.notify)(Update::Done(said)),
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => (self.notify)(Update::Failed(format!("Not done: {error}"))),
        }
        self.list(session).await
    }

    /// Sends `built`, Task `task`, in the background, so that reading goes on meanwhile; then asks for the copy in
    /// Sent, if it's `kept` and the Account keeps one, and the answered flag.
    fn send(&self, task: Task, built: Built, answers: Option<(String, u32)>, kept: bool) {
        let (account, credentials, notify, own) = (
            Arc::clone(&self.account),
            Arc::clone(&self.credentials),
            Arc::clone(&self.notify),
            self.own.clone(),
        );
        tokio::spawn(async move {
            let sent = async {
                let signing_in = credentials.credentials().await.map_err(Failure::why)?;
                smtp::send(&account.smtp, &account.trust, &signing_in, &built)
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            if sent.is_ok() {
                if kept && account.copy_to_sent {
                    own.send(Request::Append {
                        role: Role::Sent,
                        message: built.kept,
                        draft: false,
                    });
                }
                if let Some((mailbox, uid)) = answers {
                    own.send(Request::Flag {
                        mailbox,
                        uids: vec![uid],
                        flag: Flag::Answered,
                        on: true,
                    });
                }
            }
            notify(Update::Sent { task, sent });
            notify(Update::Finished(task));
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
        self.gmail = session.is_gmail();
        self.labels = label::of(&self.mailboxes, self.gmail);
        let labels: HashSet<&str> = self
            .labels
            .iter()
            .map(|label| label.mailbox.as_str())
            .collect();
        self.members
            .retain(|mailbox, _| labels.contains(mailbox.as_str()));
        (self.notify)(Update::Labels(self.labels.clone()));
        Ok(())
    }

    /// Finds out which Messages each Label's Mailbox has, where it changed since it was last found out, and where
    /// the server doesn't tell Gmail's labels: whether any had. It opens those Mailboxes.
    async fn find_members(&mut self, session: &mut Session) -> Result<bool, imap::Error> {
        if self.gmail {
            return Ok(false);
        }
        let mut changed = false;
        for label in self.labels.clone() {
            match self.find_members_of(session, &label.mailbox).await {
                Ok(found) => changed |= found,
                // A Label's Mailbox gone since it was listed isn't the connection failing.
                Err(imap::Error::Protocol(async_imap::error::Error::No(why))) => {
                    tracing::info!(
                        label = label.name,
                        why,
                        "a Label's Messages couldn't be listed"
                    );
                }
                Err(error) => return Err(error),
            }
        }
        Ok(changed)
    }

    /// Finds out which Messages the Label's Mailbox `mailbox` has, if it changed since it was last found out:
    /// whether it had.
    async fn find_members_of(
        &mut self,
        session: &mut Session,
        mailbox: &str,
    ) -> Result<bool, imap::Error> {
        let now = session.status(mailbox).await?;
        if self
            .members
            .get(mailbox)
            .is_some_and(|members| members.is_current(now))
        {
            return Ok(false);
        }
        let examined = session.examine(mailbox).await?;
        self.selected = Some(mailbox.to_owned());
        self.selected_modseq = examined.highest_modseq;
        let uids = session.uids().await?;
        let members = self.members.entry(mailbox.to_owned()).or_default();
        let lacking = members.keep(examined.uid_validity, &uids);
        if !lacking.is_empty() {
            for (uid, message_id) in session.message_ids(&imap::uid_set(&lacking)).await? {
                members.add(uid, message_id);
            }
        }
        members.listed = Some(now);
        Ok(true)
    }

    /// Gives `mailbox`'s Envelopes listed the Labels they have, as found out: Gmail's come with them.
    fn label_window(&mut self, mailbox: &str) {
        if let Some(window) = self.windows.get_mut(mailbox) {
            labelled(
                self.gmail,
                &self.labels,
                &self.members,
                window.envelopes.values_mut(),
            );
        }
    }

    /// Adds the Label whose Mailbox is `label` to the Messages `uids` of `mailbox`, or takes it off them: on Gmail by
    /// its labels, elsewhere by copying them to the Label's Mailbox, or expunging their copies there.
    async fn label(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        uids: &[u32],
        label: &str,
        on: bool,
    ) -> Result<(), imap::Error> {
        let Some(label) = self
            .labels
            .iter()
            .find(|known| known.mailbox == label)
            .cloned()
        else {
            (self.notify)(Update::Failed(format!(
                "There's no Label {}",
                crate::mailbox::decode(label)
            )));
            self.tell_window(mailbox);
            return Ok(());
        };
        let labelled = if self.gmail {
            self.select(session, mailbox).await?;
            session.set_gmail_label(uids, &label.mailbox, on).await
        } else {
            self.label_by_copies(session, mailbox, uids, &label, on)
                .await
        };
        match labelled {
            Ok(()) if on => (self.notify)(Update::Done(format!(
                "Labelled {} {}",
                count(uids.len()),
                label.name
            ))),
            Ok(()) => (self.notify)(Update::Done(format!(
                "Took {} off {}",
                label.name,
                count(uids.len())
            ))),
            Err(error) if !error.is_fatal() => {
                (self.notify)(Update::Failed(format!("Labelling: {error}")));
            }
            Err(error) => return Err(error),
        }
        if !self.gmail {
            self.find_members_of(session, &label.mailbox).await?;
        }
        // The window's Mailbox, which refreshing it takes to be open.
        self.select(session, mailbox).await?;
        self.refresh_window(session, mailbox).await?;
        self.count(session, &label.mailbox).await
    }

    /// Adds `label` to the Messages `uids` of `mailbox` by copying those that lack it to its Mailbox, or takes it
    /// off them by expunging their copies there, found by their `Message-ID`s.
    async fn label_by_copies(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        uids: &[u32],
        label: &Label,
        on: bool,
    ) -> Result<(), imap::Error> {
        self.select(session, mailbox).await?;
        let ids: HashMap<u32, String> = session
            .message_ids(&imap::uid_set(uids))
            .await?
            .into_iter()
            .filter_map(|(uid, id)| Some((uid, id?)))
            .collect();
        // What's there now, rather than when it was last found out.
        self.find_members_of(session, &label.mailbox).await?;
        let members = self
            .members
            .get(&label.mailbox)
            .cloned()
            .unwrap_or_default();
        let theirs = members.message_ids();
        if on {
            let lacking: Vec<u32> = uids
                .iter()
                .copied()
                .filter(|uid| ids.get(uid).is_none_or(|id| !theirs.contains(id.as_str())))
                .collect();
            if lacking.is_empty() {
                return Ok(());
            }
            self.select(session, mailbox).await?;
            return session.copy(&lacking, &label.mailbox).await;
        }
        if ids.is_empty() {
            return Err(imap::Error::Missing("Message-ID, to find their copies by"));
        }
        let copies = members.uids_of(&ids.values().map(String::as_str).collect());
        if copies.is_empty() {
            return Ok(());
        }
        self.select(session, &label.mailbox).await?;
        session.expunge(&copies).await?;
        if let Some(members) = self.members.get_mut(&label.mailbox) {
            members.forget(&copies);
        }
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
        self.selected_modseq = selected.highest_modseq;
        Ok(selected)
    }

    /// Opens `mailbox`, and tells its Envelopes: the newest, the first time; what's new and changed since, after.
    async fn open(&mut self, session: &mut Session, mailbox: &str) -> Result<(), imap::Error> {
        self.open_as(session, mailbox, false).await
    }

    /// Opens `mailbox`: lists its newest Messages' Envelopes, as when it's first opened or if `anew`, else brings
    /// those listed up to date. Listed anew, what was listed stays until what replaces it is had: the connection
    /// dropping meanwhile leaves it as it was.
    async fn open_as(
        &mut self,
        session: &mut Session,
        mailbox: &str,
        anew: bool,
    ) -> Result<(), imap::Error> {
        // What the cache keeps is brought up to date, rather than fetched again.
        if !anew {
            self.listed_from_cache(mailbox);
        }
        let selected = self.select(session, mailbox).await?;
        let window = self.windows.entry(mailbox.to_owned()).or_default();
        if anew || window.uid_validity != selected.uid_validity || window.envelopes.is_empty() {
            *window = Window {
                uid_validity: selected.uid_validity,
                exists: selected.exists,
                highest_modseq: selected.highest_modseq,
                envelopes: session
                    .newest(selected.exists, WINDOW)
                    .await?
                    .into_iter()
                    .map(|envelope| (envelope.uid, envelope))
                    .collect(),
            };
        } else {
            window.exists = selected.exists;
            Self::update_window(session, window, selected.highest_modseq).await?;
        }
        self.label_window(mailbox);
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
        // Last, so that what's new shows first: finding out takes a while the first time, and opens the Labels'
        // Mailboxes.
        if self.find_members(session).await? {
            self.label_window(mailbox);
            self.tell_window(mailbox);
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
        let mut envelopes = if newest.is_empty() {
            Vec::new()
        } else {
            session.envelopes(&imap::uid_set(newest)).await?
        };
        labelled(self.gmail, &self.labels, &self.members, &mut envelopes);
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
        self.label_window(mailbox);
        self.tell_window(mailbox);
        Ok(())
    }

    /// The Envelopes of what's new in `window`'s Mailbox, which is selected, and the Flags of what it has.
    /// Brings `window` up to date with its Mailbox, selected, whose highest mod-sequence when it was is `now`, if
    /// the server keeps them: what's new, and what's changed since it was last brought up to date (only that, by
    /// CONDSTORE, when both mod-sequences are known), and what's gone.
    async fn update_window(
        session: &mut Session,
        window: &mut Window,
        now: Option<u64>,
    ) -> Result<(), imap::Error> {
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
        let listed = format!("{first}:{last}");
        let changed: Option<HashSet<u32>> = match (window.highest_modseq, now) {
            // What's still there, and the Flags of what changed since: not every one's.
            (Some(since), Some(_)) if session.can_condstore() => {
                let there: HashSet<u32> = session.uids_in(&listed).await?.into_iter().collect();
                window
                    .envelopes
                    .retain(|uid, _| *uid > last || there.contains(uid));
                let flags = session.changed_flags(&listed, since).await?;
                for (uid, flags) in &flags {
                    if let Some(envelope) = window.envelopes.get_mut(uid) {
                        envelope.flags = *flags;
                    }
                }
                Some(flags.into_iter().map(|(uid, _)| uid).collect())
            }
            _ => {
                let flags: HashMap<u32, Flags> =
                    session.flags(&listed).await?.into_iter().collect();
                // What's no longer there was expunged.
                window
                    .envelopes
                    .retain(|uid, _| *uid > last || flags.contains_key(uid));
                for (uid, envelope) in &mut window.envelopes {
                    if let Some(flags) = flags.get(uid) {
                        envelope.flags = *flags;
                    }
                }
                None
            }
        };
        // Taken from a SELECT, before what was asked since: what changed meanwhile is asked again, never missed.
        if now.is_some() {
            window.highest_modseq = now;
        }
        // Those listed before Gmail's Threads were asked for, as the cache may keep them.
        session
            .add_gmail_threads(window.envelopes.values_mut())
            .await?;
        // Gmail changes a Message's mod-sequence with its labels: only what changed is asked again.
        session
            .refresh_gmail_labels(window.envelopes.values_mut().filter(|envelope| {
                envelope.uid > last
                    || changed
                        .as_ref()
                        .is_none_or(|changed| changed.contains(&envelope.uid))
            }))
            .await?;
        Ok(())
    }

    async fn refresh_window(
        &mut self,
        session: &mut Session,
        mailbox: &str,
    ) -> Result<(), imap::Error> {
        let now = if self.selected.as_deref() == Some(mailbox) {
            self.selected_modseq
        } else {
            None
        };
        if let Some(window) = self.windows.get_mut(mailbox) {
            Self::update_window(session, window, now).await?;
            self.label_window(mailbox);
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
                highest_modseq: window.highest_modseq,
            };
            self.cache.keep_envelopes(mailbox, &listed);
            (self.notify)(Update::Envelopes {
                mailbox: mailbox.to_owned(),
                envelopes: listed.envelopes,
                exists: listed.exists,
            });
        }
    }

    /// Tells the UI the Envelopes of `mailbox` listed, again, as they stand: once a Task that changed them is done.
    fn tell_window_again(&self, mailbox: &str) {
        if let Some(window) = self.windows.get(mailbox) {
            (self.notify)(Update::Envelopes {
                mailbox: mailbox.to_owned(),
                envelopes: window.envelopes.values().cloned().collect(),
                exists: window.exists,
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
                    highest_modseq: listed.highest_modseq,
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

    /// Searches every Mailbox that can be opened for `query`, and tells how many Messages each has that it finds.
    async fn search_all(
        &mut self,
        session: &mut Session,
        query: String,
    ) -> Result<(), imap::Error> {
        let names: Vec<String> = self
            .mailboxes
            .iter()
            .filter(|mailbox| mailbox.selectable)
            .map(|mailbox| mailbox.name.clone())
            .collect();
        let mut found = Vec::new();
        for name in names {
            // Examined, only to read: what's searched stays as it is.
            let searched = async {
                session.examine(&name).await?;
                session.search(&query, self.gmail).await
            }
            .await;
            match searched {
                Ok(uids) if !uids.is_empty() => found.push((name, uids.len())),
                Ok(_) => {}
                Err(error) if !error.is_fatal() => {
                    tracing::info!(mailbox = name, %error, "a Mailbox couldn't be searched");
                }
                Err(error) => return Err(error),
            }
        }
        // Examined, none is selected: the next to read or change is selected again.
        self.selected = None;
        (self.notify)(Update::FoundIn { query, found });
        Ok(())
    }

    /// The Mailbox named `name` on the server, by its name for people.
    fn named(&self, name: &str) -> String {
        self.mailboxes
            .iter()
            .find(|known| known.name == name)
            .map_or_else(
                || crate::mailbox::decode(name),
                |known| known.label().to_owned(),
            )
    }

    /// The Role of the Mailbox archiving moves Messages to: All Mail on Gmail, which archives by taking them out of
    /// the Inbox; the Archive elsewhere.
    fn archive_role(&self) -> Role {
        if self.account.gmail {
            Role::All
        } else {
            Role::Archive
        }
    }

    /// The Task tried again that's due, if one is, and how many times it was tried.
    fn due_again(&mut self) -> Option<(Task, Request, usize)> {
        let now = tokio::time::Instant::now();
        let at = self.again.iter().position(|(due, ..)| *due <= now)?;
        let (_, task, request, tries) = self.again.remove(at);
        Some((task, request, tries))
    }

    /// Waits until `due`, or for ever if nothing is.
    async fn until_again(due: Option<tokio::time::Instant>) {
        match due {
            Some(due) => tokio::time::sleep_until(due).await,
            None => std::future::pending().await,
        }
    }

    /// Does `task`, a [`Request::MoveBack`] tried `tries` times before: if the Messages aren't where they went
    /// yet, it's tried again once it's waited, until it's waited the last time. Told finished once it's done with,
    /// moved back (as [`Update::MovedBack`] says) or not.
    async fn move_back_task(
        &mut self,
        session: &mut Session,
        task: Task,
        request: Request,
        tries: usize,
    ) -> Result<(), imap::Error> {
        let Request::MoveBack {
            went,
            to,
            message_ids,
        } = &request
        else {
            return Ok(());
        };
        if tries == 0 {
            (self.notify)(Update::Started(task));
        }
        let moved = match self.move_back(session, went, to, message_ids).await {
            Ok(moved) => moved,
            Err(error) => {
                (self.notify)(Update::Finished(task));
                return self.tolerate(Err(error));
            }
        };
        match moved {
            MovedBack::NotThere(_) if tries < MOVE_BACK_WAITS.len() => {
                let due = tokio::time::Instant::now() + MOVE_BACK_WAITS[tries];
                self.again.push((due, task, request, tries + 1));
                return Ok(());
            }
            MovedBack::Moved => (self.notify)(Update::MovedBack(task)),
            MovedBack::NotThere(from) => (self.notify)(Update::Failed(format!(
                "Not moved back: they aren't in {from}"
            ))),
            MovedBack::Refused(why) => (self.notify)(Update::Failed(why)),
        }
        (self.notify)(Update::Finished(task));
        Ok(())
    }

    /// Moves the Messages with `message_ids` in the Mailbox they `went` to back `to` where they were.
    async fn move_back(
        &mut self,
        session: &mut Session,
        went: &Went,
        to: &str,
        message_ids: &[String],
    ) -> Result<MovedBack, imap::Error> {
        let from = match went {
            Went::Mailbox(name) => Some(name.clone()),
            Went::Trash => self.with_role(Role::Trash),
            Went::Archived => self.with_role(self.archive_role()),
            Went::Junk => self.with_role(Role::Junk),
        };
        let Some(from) = from else {
            return Ok(MovedBack::Refused("Nowhere to move them back from".into()));
        };
        // Selected again, so that what's come since it was shows.
        self.selected = None;
        self.select(session, &from).await?;
        let uids = session.with_message_ids(message_ids).await?;
        if uids.is_empty() {
            return Ok(MovedBack::NotThere(self.named(&from)));
        }
        // Taken out of All Mail, a Gmail Message is deleted: one archived goes back to the Inbox as a copy, which
        // labels it so again.
        let moved = if self.with_role(Role::All).as_deref() == Some(from.as_str()) {
            session.copy(&uids, to).await
        } else {
            session.move_to(&uids, to).await
        };
        let outcome = match moved {
            Ok(()) => {
                (self.notify)(Update::Done(format!(
                    "Moved {} back to {}",
                    count(uids.len()),
                    self.named(to)
                )));
                MovedBack::Moved
            }
            Err(error) if error.is_fatal() => return Err(error),
            Err(error) => MovedBack::Refused(format!("Moving back: {error}")),
        };
        for mailbox in [from.as_str(), to] {
            if self.windows.contains_key(mailbox) {
                self.select(session, mailbox).await?;
                self.refresh_window(session, mailbox).await?;
            }
            self.count(session, mailbox).await?;
        }
        Ok(outcome)
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

/// What came of moving Messages back.
enum MovedBack {
    Moved,
    /// They aren't in the Mailbox they went to, by its name for people: not yet, or no longer.
    NotThere(String),
    /// The server wouldn't, for this reason.
    Refused(String),
}

/// The next Task to do of those `queue`d: a Message to read before anything else, as reading waits for it, and
/// reading only, so it comes to no harm for coming first; else the first asked.
fn next(queue: &mut VecDeque<(Task, Request)>) -> Option<(Task, Request)> {
    let reading = queue
        .iter()
        .position(|(_, request)| matches!(request, Request::Body { .. }));
    match reading {
        Some(at) => queue.remove(at),
        None => queue.pop_front(),
    }
}

/// Gives `envelopes` the Labels whose Mailboxes have them, by `members`, once what each has is found out; unless the
/// server tells Gmail's labels (`gmail`), which come with them. Until then, they keep what they have, as cached.
fn labelled<'a>(
    gmail: bool,
    labels: &[Label],
    members: &HashMap<String, Members>,
    envelopes: impl IntoIterator<Item = &'a mut Envelope>,
) {
    if gmail {
        return;
    }
    let found: Option<Vec<(&Label, &Members)>> = labels
        .iter()
        .map(|label| Some((label, members.get(&label.mailbox)?)))
        .collect();
    if let Some(found) = found {
        label::label(envelopes, &found);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::OAuth2;

    #[test]
    fn a_message_to_read_goes_first_and_the_rest_in_the_order_asked() {
        let mut queue: VecDeque<(Task, Request)> = [
            (
                Task(1),
                Request::Delete {
                    mailbox: "INBOX".into(),
                    uids: vec![1],
                },
            ),
            (Task(2), Request::Open("Work".into())),
            (
                Task(3),
                Request::Body {
                    mailbox: "INBOX".into(),
                    uid: 2,
                },
            ),
            (Task(4), Request::Refresh),
        ]
        .into();
        let order: Vec<u64> = std::iter::from_fn(|| next(&mut queue))
            .map(|(Task(number), _)| number)
            .collect();
        assert_eq!(order, [3, 1, 2, 4]);
    }

    #[test]
    fn envelopes_keep_their_labels_until_every_labels_messages_are_found_out() {
        let label = |name: &str| Label {
            name: name.to_owned(),
            mailbox: format!("Labels/{name}"),
        };
        let labels = [label("GH"), label("RfL")];
        let mut gh = Members::default();
        gh.keep(1, &[1]);
        gh.add(1, Some("a@x".into()));
        let mut members = HashMap::from([("Labels/GH".to_owned(), gh)]);
        let mut envelopes = [Envelope {
            message_id: Some("a@x".into()),
            labels: vec!["RfL".into()],
            ..Envelope::read(1, Flags::default(), None, 0, b"")
        }];
        labelled(false, &labels, &members, &mut envelopes);
        assert_eq!(
            envelopes[0].labels,
            ["RfL"],
            "as cached: RfL's aren't known yet"
        );
        members.insert("Labels/RfL".to_owned(), Members::default());
        labelled(true, &labels, &members, &mut envelopes);
        assert_eq!(
            envelopes[0].labels,
            ["RfL"],
            "Gmail's come with the Envelopes"
        );
        labelled(false, &labels, &members, &mut envelopes);
        assert_eq!(envelopes[0].labels, ["GH"]);
    }

    /// A client of the token endpoint at `token_url`.
    fn client_of(token_url: String) -> OAuth2 {
        OAuth2 {
            client_id: "client".into(),
            client_secret: None,
            auth_url: "https://accounts.example/auth".into(),
            token_url,
            scopes: vec!["https://mail.google.com/".into()],
        }
    }

    #[tokio::test]
    async fn not_reaching_the_provider_is_tried_again_but_its_refusal_waits() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A port nothing listens on, as before the network is back on waking.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let unreachable = client_of(format!("http://127.0.0.1:{port}/token"));
        let error = oauth2::refresh(&unreachable, None, "refresh")
            .await
            .unwrap_err();
        assert!(matches!(Failure::refreshing(&error), Failure::Network(_)));

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant", "error_description": "Token has been expired or revoked."
            })))
            .mount(&server)
            .await;
        let refusing = client_of(format!("{}/token", server.uri()));
        let error = oauth2::refresh(&refusing, None, "refresh")
            .await
            .unwrap_err();
        assert!(matches!(
            Failure::refreshing(&error),
            Failure::SignIn(why) if why.contains("invalid_grant")
        ));
    }
}
