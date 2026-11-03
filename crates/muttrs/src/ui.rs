//! The terminal UI: what's on screen, the keys, and the loop that draws it as events come.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use base64::Engine as _;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TerminalEvent, EventStream, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt as _;
use muttrs_gfx::{CellSize, Prepared, Protocol};
use muttrs_mail::client::{self, Client, Connection, Flag, Task, Update};
use muttrs_mail::{Account, Message, Role};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr as _;

use crate::addresses::AddressBook;
use crate::assessment::{self, Assessed, Assessments, Assessor, Readiness, Window};
use crate::compose::{self, Compose};
use crate::config::Config;
use crate::filter::{self, Filter};
use crate::history::{self, History, Recall};
use crate::icons::Icons;
use crate::index::{self, Index};
use crate::login;
use crate::overlay::{self, Outcome, Overlay, Verb};
use crate::pager::{self, Pager, Request};
use crate::pictures::{self, Pictures};
use crate::remote::Fetcher;
use crate::sidebar::{self, AccountState, Row, Sidebar};
use crate::sign_in::{self, SignIn};
use crate::tasks;
use crate::theme::Theme;
use crate::xdg;

/// What happens in the background, for the UI to take in.
pub(crate) enum Event {
    /// A Picture was prepared, or couldn't be.
    Picture(pictures::Key, Result<Prepared, String>),
    /// Remote Content was fetched for image `image` of the Pager `pager`, or couldn't be.
    Fetched {
        pager: u64,
        image: usize,
        bytes: Option<Vec<u8>>,
    },
    /// What an Account says, by its place in the sidebar.
    Account(usize, Update),
    /// What voight says.
    Voight(assessment::Event),
    /// The Pager, by its ID, has shown its Message for `pager_read_delay`.
    Read(u64),
    /// How signing in to an Account again goes, by the sign-in's ID.
    SignIn(u64, sign_in::Progress),
}

/// What the UI is set up with.
pub(crate) struct Setup {
    pub(crate) config: Config,
    pub(crate) theme: Theme,
    pub(crate) icons: Icons,
    pub(crate) protocol: Option<Protocol>,
    /// voight, started, if it's on.
    pub(crate) assessor: Option<Assessor>,
}

/// A line typed at the foot of the screen: a search, or a command.
struct Prompt {
    kind: PromptKind,
    text: String,
    /// The completions `Tab` goes through, while it does.
    completing: Option<Completing>,
    /// Going back through the History, while `Up`, `Down`, `Ctrl-P` or `Ctrl-N` do.
    recalling: Option<Recall>,
}

impl Prompt {
    fn new(kind: PromptKind, text: String) -> Self {
        Self {
            kind,
            text,
            completing: None,
            recalling: None,
        }
    }
}

/// Completions of the address being typed.
struct Completing {
    /// What's before it: the addresses typed already.
    before: String,
    candidates: Vec<String>,
    /// The one shown.
    at: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Search,
    Command,
    /// The path of a file to attach to the Draft.
    Attach,
    /// What to search the server for.
    Server,
    /// Whom a new Message is to, before its subject is asked.
    To,
    /// A new Message's subject, after whom it's to.
    Subject,
}

impl PromptKind {
    /// The History of what's typed at it, if it has one.
    fn history(self) -> Option<history::Kind> {
        match self {
            Self::Command => Some(history::Kind::Command),
            Self::Search | Self::Server => Some(history::Kind::Search),
            Self::Attach | Self::To | Self::Subject => None,
        }
    }
}

/// What `u` undoes: Messages moved, archived or deleted to Trash, moved back from where they went.
struct Undo {
    account: usize,
    went: client::Went,
    /// Where they were, by its name on the server.
    from: String,
    /// Theirs: they're found by them where they went, with UIDs of their own there.
    message_ids: Vec<String>,
}

/// How many moves `u` can undo, the last first.
const UNDOES: usize = 100;

/// What's being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writing {
    New,
    Reply,
    ReplyAll,
    Forward,
    /// A Postponed Draft, resumed.
    Resume,
}

/// Which pane keys go to, when the Pager isn't open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Index,
}

/// A Message of an Account's Mailbox, by its UID.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Opened {
    account: usize,
    mailbox: String,
    uid: u32,
}

impl Opened {
    /// Whether `index` lists its Mailbox.
    fn is_in(&self, index: &Index) -> bool {
        index.account == self.account && index.mailbox == self.mailbox
    }
}

/// A word for the user at the foot of the screen, until the next key.
struct Notice {
    text: String,
    error: bool,
}

// The flags are each what the UI is doing, not a state machine folded into bools.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct App {
    config: Config,
    theme: Theme,
    icons: Icons,
    pictures: Pictures,
    fetcher: Option<Fetcher>,
    events: mpsc::UnboundedSender<Event>,
    accounts: Vec<AccountState>,
    sidebar: Sidebar,
    sidebar_shown: bool,
    focus: Focus,
    index: Option<Index>,
    /// The Mailbox's Index, while the Index shows what a search found in it.
    searched: Option<Index>,
    pager: Option<Pager>,
    /// The Message the Pager shows, or is waiting for.
    opened: Option<Opened>,
    /// The Pager, by its ID, whose Message is marked read once it's shown for `pager_read_delay`.
    reading: Option<u64>,
    overlay: Option<Overlay>,
    prompt: Option<Prompt>,
    /// Signing in to an Account again, in the browser, over everything else.
    sign_in: Option<SignIn>,
    notice: Option<Notice>,
    next_pager: u64,
    /// Whether the Pager is all there is, as `muttrs view` opens it.
    standalone: bool,
    /// The Draft being written.
    compose: Option<Compose>,
    /// The Drafts being sent, by their Tasks: put away meanwhile, and shown again if they can't be sent.
    sending: Vec<(Task, Compose)>,
    /// The Drafts that couldn't be sent, shown again once no other is.
    unsent: Vec<Compose>,
    /// How many Drafts this run has started, to name their files.
    drafts: u64,
    /// Whether the Draft is to be edited in `$EDITOR`, which the loop does between draws.
    edit: bool,
    /// What to write once the Message being fetched comes, as when replying from the Index.
    write_when_fetched: Option<Writing>,
    /// The first key of a pair typed, as `z` of `za`.
    pending: Option<char>,
    /// Whom the new Message is to, while its subject is asked.
    writing_to: Option<Vec<muttrs_mail::Address>>,
    /// The addresses seen, to complete with.
    addresses: AddressBook,
    /// What was typed at the prompt before.
    history: History,
    /// Where the sidebar and the Index were last drawn, for the mouse.
    drawn: (Rect, Rect),
    /// voight, to assess the Pager's prose, if it's on; and the Assessments it made.
    assessor: Option<Assessor>,
    assessments: Option<Assessments>,
    /// Whether the Pager shows its prose as the Heatmap of its Assessment.
    heatmap: bool,
    /// What `u` undoes, the last done last.
    undo: Vec<Undo>,
    /// When the UI started: the status line's spinner turns by it.
    started: Instant,
    /// Whether to quit once the Accounts' Tasks are done, as quitting asked; until a key's pressed.
    quitting: bool,
    quit: bool,
}

impl App {
    fn new(
        setup: Setup,
        events: mpsc::UnboundedSender<Event>,
        accounts: Vec<AccountState>,
        standalone: bool,
    ) -> Self {
        let mut assessor = setup.assessor;
        if let Some(mut said) = assessor.as_mut().and_then(Assessor::events) {
            let events = events.clone();
            tokio::spawn(async move {
                while let Some(event) = said.recv().await {
                    if events.send(Event::Voight(event)).is_err() {
                        return;
                    }
                }
            });
        }
        let assessments = assessor.is_some().then(Assessments::default);
        let fetcher = match Fetcher::new() {
            Ok(fetcher) => Some(fetcher),
            Err(error) => {
                tracing::warn!(%error, "Remote Content can't be fetched");
                None
            }
        };
        Self {
            pictures: Pictures::new(setup.protocol, cell_size(), events.clone()),
            config: setup.config,
            theme: setup.theme,
            icons: setup.icons,
            fetcher,
            events,
            accounts,
            sidebar: Sidebar::default(),
            sidebar_shown: true,
            focus: Focus::Index,
            index: None,
            searched: None,
            pager: None,
            opened: None,
            reading: None,
            overlay: None,
            prompt: None,
            sign_in: None,
            notice: None,
            next_pager: 1,
            standalone,
            compose: None,
            sending: Vec::new(),
            unsent: Vec::new(),
            drafts: 0,
            edit: false,
            write_when_fetched: None,
            pending: None,
            writing_to: None,
            addresses: if standalone {
                AddressBook::default()
            } else {
                AddressBook::load(xdg::cache_dir().map(|cache| cache.join("addresses.txt")))
            },
            history: History::load(xdg::state_dir().map(|state| state.join("history"))),
            drawn: (Rect::default(), Rect::default()),
            assessor,
            assessments,
            heatmap: false,
            undo: Vec::new(),
            started: Instant::now(),
            quitting: false,
            quit: false,
        }
    }

    /// Asks voight for the Assessment of the prose the Pager shows, once it's ready and assessing nothing else.
    fn start_assessment(&mut self) {
        let (Some(assessor), Some(assessments)) = (&self.assessor, &mut self.assessments) else {
            return;
        };
        let Some(prose) = self.pager.as_ref().and_then(Pager::prose) else {
            return;
        };
        if !assessments.can_assess() || assessments.get(prose).is_some() {
            return;
        }
        assessments.assessing(Arc::clone(prose));
        assessor.assess(Arc::clone(prose));
    }

    /// Takes in what voight says: that its model is loaded, or why it couldn't be, or an Assessment.
    fn voight_said(&mut self, event: assessment::Event) {
        let Some(assessments) = &mut self.assessments else {
            return;
        };
        match event {
            assessment::Event::Loaded { fallback } => {
                assessments.loaded(true);
                if let Some(fallback) = fallback {
                    self.say(&fallback, false);
                }
            }
            assessment::Event::Failed(why) => {
                assessments.loaded(false);
                self.say(&format!("voight couldn't load its model: {why}"), true);
            }
            assessment::Event::Assessed { key, assessment } => {
                assessments.assessed(key, assessment);
            }
        }
    }

    /// What the status line says of voight while the Pager shows: that it's loading, assessing, or its Assessment,
    /// in the Heatmap's colour for it while that shows; `None` when there's nothing to say, as for a body with too
    /// little prose.
    fn assessment_label(&self) -> Option<Span<'static>> {
        let assessments = self.assessments.as_ref()?;
        let prose = self.pager.as_ref()?.prose()?;
        let said = |label: &str| Some(Span::styled(label.to_owned(), self.theme.voight));
        match assessments.readiness() {
            Readiness::Loading => return said("voight loading…"),
            Readiness::Failed => return None,
            Readiness::Ready => {}
        }
        match assessments.get(prose) {
            None | Some(Assessed::Assessing) => said("voight …"),
            Some(Assessed::Done(Some(assessment))) => {
                let style = if self.heatmap {
                    self.theme.voight.patch(self.theme.heat(assessment.share))
                } else {
                    self.theme.voight
                };
                let label = format!("voight {:.0}%", assessment.share * 100.0);
                Some(Span::styled(label, style))
            }
            Some(Assessed::Done(None)) => None,
            Some(Assessed::Failed(_)) => said("voight failed"),
        }
    }

    /// The windows the Pager's prose is shaded by, while the Heatmap shows: its Assessment's, once made.
    fn heat(&self) -> Option<Arc<[Window]>> {
        if !self.heatmap {
            return None;
        }
        match self
            .assessments
            .as_ref()?
            .get(self.pager.as_ref()?.prose()?)?
        {
            Assessed::Done(Some(assessment)) => Some(Arc::clone(&assessment.windows)),
            Assessed::Done(None) | Assessed::Assessing | Assessed::Failed(_) => None,
        }
    }

    /// Shows the Heatmap, or stops; says why when it can't show now.
    fn toggle_heatmap(&mut self) {
        if self.assessments.is_none() {
            self.say("No Heatmap: voight is off", false);
            return;
        }
        self.heatmap = !self.heatmap;
        if self.heatmap && self.heat().is_none() {
            self.say("The Heatmap shows on prose voight has assessed", false);
        }
    }

    /// Opens `message` in the Pager.
    fn open(&mut self, message: Arc<Message>) {
        // "Fetching the Message…" is done with.
        self.notice = None;
        self.addresses.learn(
            message
                .from
                .iter()
                .chain(&message.to)
                .chain(&message.cc)
                .chain(&message.reply_to),
        );
        let id = self.next_pager;
        self.next_pager += 1;
        let mut pager = Pager::new(
            id,
            message,
            &self.theme,
            self.config.prefer,
            self.config.load_remote_content,
        );
        let request = pager.remote_to_fetch();
        self.pager = Some(pager);
        self.read_later(id);
        self.fulfil(request);
    }

    /// Marks the opened Message read once the Pager `pager` has shown it for `pager_read_delay`, if it's Unread.
    fn read_later(&mut self, pager: u64) {
        self.reading = None;
        let delay = self.config.pager_read_delay;
        if delay == 0
            || !self
                .opened
                .as_ref()
                .is_some_and(|opened| self.is_unread(opened))
        {
            return;
        }
        self.reading = Some(pager);
        let events = self.events.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(delay)).await;
            let _ = events.send(Event::Read(pager));
        });
    }

    /// Whether the Index lists `opened`, Unread.
    fn is_unread(&self, opened: &Opened) -> bool {
        self.index
            .as_ref()
            .filter(|index| opened.is_in(index))
            .and_then(|index| index.find(opened.uid))
            .is_some_and(|envelope| !envelope.flags.seen)
    }

    /// Marks `opened` read, in the Index, its Mailbox's counts and on the server, if it's Unread.
    fn mark_seen(&mut self, opened: &Opened) {
        let Some(envelope) = self
            .index
            .as_mut()
            .filter(|index| opened.is_in(index))
            .and_then(|index| index.find_mut(opened.uid))
        else {
            return;
        };
        if envelope.flags.seen {
            return;
        }
        envelope.flags.seen = true;
        let Some(state) = self.accounts.get_mut(opened.account) else {
            return;
        };
        state.ask(client::Request::Flag {
            mailbox: opened.mailbox.clone(),
            uids: vec![opened.uid],
            flag: Flag::Seen,
            on: true,
        });
        if let Some(counts) = state.counts.get_mut(&opened.mailbox) {
            counts.unread = counts.unread.saturating_sub(1);
        }
    }

    fn fulfil(&mut self, request: Request) {
        let Request::Fetch(images) = request else {
            return;
        };
        let (Some(fetcher), Some(pager)) = (&self.fetcher, &self.pager) else {
            return;
        };
        let pager = pager.id();
        for (image, url) in images {
            let (fetcher, events) = (fetcher.clone(), self.events.clone());
            tokio::spawn(async move {
                let bytes = match fetcher.image(&url).await {
                    Ok(bytes) => Some(bytes),
                    Err(error) => {
                        tracing::debug!(%error, "Remote Content couldn't be fetched");
                        None
                    }
                };
                let _ = events.send(Event::Fetched {
                    pager,
                    image,
                    bytes,
                });
            });
        }
    }

    fn on_event(&mut self, event: Event, out: &mut impl Write) -> io::Result<()> {
        match event {
            Event::Picture(key, prepared) => self.pictures.prepared(key, prepared, out)?,
            Event::Fetched {
                pager,
                image,
                bytes,
            } => {
                if let Some(open) = self.pager.as_mut().filter(|open| open.id() == pager) {
                    open.fetched(image, bytes);
                }
            }
            Event::Account(account, update) => self.on_account(account, update),
            Event::SignIn(id, progress) => self.signing_in(id, progress),
            Event::Voight(said) => self.voight_said(said),
            Event::Read(pager) => {
                if self.reading == Some(pager) {
                    self.reading = None;
                    if let Some(opened) = self.opened.clone() {
                        self.mark_seen(&opened);
                    }
                }
            }
        }
        Ok(())
    }

    fn on_account(&mut self, account: usize, update: Update) {
        let Some(state) = self.accounts.get_mut(account) else {
            return;
        };
        match update {
            Update::Connection(connection) => {
                match &connection {
                    Connection::SignInFailed(why) => {
                        let name = state.name.clone();
                        // The hint goes before the reason, which the status line may cut short.
                        let text = if self.signs_in_with_oauth2(&name) {
                            format!(
                                "{name}: signing in failed (`:login {name}` signs in again): {why}"
                            )
                        } else {
                            format!("{name}: signing in failed: {why}")
                        };
                        self.say(&text, true);
                    }
                    Connection::Offline(why) if state.connection == Connection::Online => {
                        let text = format!("{}: offline: {why}", state.name);
                        self.say(&text, true);
                    }
                    _ => {}
                }
                if let Some(state) = self.accounts.get_mut(account) {
                    state.connection = connection;
                }
            }
            Update::Mailboxes(mailboxes) => {
                state.mailboxes = mailboxes;
                // The first Account's Inbox, to start with.
                if self.index.is_none()
                    && let Some(inbox) =
                        state.with_role(Role::Inbox).map(|inbox| inbox.name.clone())
                {
                    self.open_mailbox(account, &inbox);
                }
            }
            Update::Labels(labels) => state.labels = labels,
            Update::Counts { mailbox, counts } => {
                state.counts.insert(mailbox, counts);
            }
            Update::Envelopes {
                mailbox,
                envelopes,
                exists,
            } => {
                self.addresses.learn(envelopes.iter().flat_map(|envelope| {
                    envelope.from.iter().chain(&envelope.to).chain(&envelope.cc)
                }));
                // While a search's results show, the Mailbox's Index is kept up to date aside.
                let index = match &mut self.searched {
                    Some(searched) => Some(searched),
                    None => self.index.as_mut(),
                };
                if let Some(index) =
                    index.filter(|index| index.account == account && index.mailbox == mailbox)
                {
                    // As they'll be once what's asked is done: what the server tells may be from before.
                    let mut envelopes = envelopes;
                    state.tasks.apply(&mailbox, &mut envelopes);
                    index.set(envelopes, exists);
                }
            }
            Update::Found {
                mailbox,
                query,
                mut envelopes,
                count,
            } => {
                state.tasks.apply(&mailbox, &mut envelopes);
                let Some(index) = self
                    .index
                    .take_if(|index| index.account == account && index.mailbox == mailbox)
                else {
                    return;
                };
                let found = index.found(query, envelopes, count);
                if index.found.is_none() {
                    self.searched = Some(index);
                }
                self.say(
                    &match count {
                        0 => "Nothing found".to_owned(),
                        1 => "Found 1 Message".to_owned(),
                        count => format!("Found {count} Messages"),
                    },
                    false,
                );
                self.index = Some(found);
                self.focus = Focus::Index;
            }
            Update::Body { mailbox, uid, body } => {
                let wanted = Opened {
                    account,
                    mailbox,
                    uid,
                };
                if self.opened.as_ref() != Some(&wanted) {
                    return;
                }
                match body {
                    Ok(bytes) => match Message::parse(bytes.as_ref().clone()) {
                        Ok(message) => {
                            let message = Arc::new(message);
                            match self.write_when_fetched.take() {
                                Some(writing) => {
                                    self.opened = None;
                                    self.write_about(writing, account, &message, uid);
                                }
                                None => self.open(message),
                            }
                        }
                        Err(error) => self.say(&error.to_string(), true),
                    },
                    Err(why) => {
                        self.write_when_fetched = None;
                        self.say(&format!("Couldn't fetch the Message: {why}"), true);
                    }
                }
            }
            Update::Done(done) => self.say(&done, false),
            Update::Failed(failed) => self.say(&failed, true),
            Update::Started(task) => state.tasks.started(task, Instant::now()),
            Update::Finished(task) => {
                state.tasks.finished(task);
                self.quit_if_done();
            }
            Update::Sent { task, sent } => self.sent(account, task, sent),
        }
    }

    /// Starts writing: a new Message, or one about the Message the Pager shows or the Index's cursor is on.
    fn write(&mut self, writing: Writing) {
        if self.standalone {
            self.say("Writing needs an Account: run muttrs without `view`", true);
            return;
        }
        if writing == Writing::New {
            // Whom to, then what about, as mutt asks, before the editor.
            self.prompt = Some(Prompt::new(PromptKind::To, String::new()));
            return;
        }
        if let (Some(pager), Some(opened)) = (&self.pager, &self.opened) {
            let (message, account, uid) = (Arc::clone(pager.message()), opened.account, opened.uid);
            self.write_about(writing, account, &message, uid);
            return;
        }
        // From the Index: once the Message is fetched.
        self.write_when_fetched = Some(writing);
        self.open_current();
    }

    fn write_about(&mut self, writing: Writing, account: usize, message: &Message, uid: u32) {
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        let me = state.sender.clone();
        let text = compose::quotable(message);
        let mailbox = self
            .index
            .as_ref()
            .map(|index| index.mailbox.clone())
            .unwrap_or_default();
        let (draft, answers) = match writing {
            Writing::Reply | Writing::ReplyAll => (
                muttrs_mail::compose::Draft::reply(
                    message,
                    &me,
                    writing == Writing::ReplyAll,
                    &text,
                ),
                Some((mailbox, uid)),
            ),
            Writing::Forward | Writing::New => (
                muttrs_mail::compose::Draft::forward(message, &me, &text),
                None,
            ),
            Writing::Resume => {
                // A Draft kept is its Markdown as plain text, and the HTML it made.
                let markdown = message.plain().map_or(text, |(markdown, _)| markdown);
                let draft = muttrs_mail::compose::Draft::resumed(message, &markdown);
                self.start(account, draft, None);
                if let Some(compose) = &mut self.compose {
                    compose.replaces = Some((mailbox, uid));
                }
                return;
            }
        };
        self.start(account, draft, answers);
    }

    /// Opens `draft` in `$EDITOR`.
    fn start(
        &mut self,
        account: usize,
        draft: muttrs_mail::compose::Draft,
        answers: Option<(String, u32)>,
    ) {
        if self.compose.is_some() {
            self.say("A Draft is being written already", true);
            return;
        }
        self.drafts += 1;
        match Compose::new(account, draft, answers, self.drafts) {
            Ok(compose) => {
                self.compose = Some(compose);
                self.edit = true;
            }
            Err(error) => self.say(&format!("Couldn't write the Draft's file: {error}"), true),
        }
    }

    /// Takes the Draft back from `$EDITOR`, which exited as `status` says.
    fn edited(&mut self, status: std::io::Result<std::process::ExitStatus>) {
        self.notice = None;
        let Some(compose) = &mut self.compose else {
            return;
        };
        match status {
            Err(error) => {
                self.say(&format!("Couldn't run the editor: {error}"), true);
                return;
            }
            Ok(status) if !status.success() => {
                self.say(
                    &format!("The editor exited with {status}: the Draft is as it was"),
                    true,
                );
                return;
            }
            Ok(_) => {}
        }
        match compose.reread() {
            Err(error) => {
                self.say(&format!("{error:#}; e edits it again"), true);
                return;
            }
            Ok(false) => {
                if let Some(compose) = self.compose.take() {
                    compose.finish();
                }
                self.say("Not sent: the Draft is as it was", false);
                return;
            }
            Ok(true) => {}
        }
        if compose.draft.is_empty() {
            if let Some(compose) = self.compose.take() {
                compose.finish();
            }
            self.say("Not sent: the Draft says nothing", false);
        }
    }

    /// Keys while a Draft is shown.
    fn compose_key(&mut self, key: KeyEvent) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let discarding = std::mem::take(&mut compose.discarding);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => compose.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => compose.scroll_by(-1),
            KeyCode::Char('e') => self.edit = true,
            KeyCode::Char('y') => self.send(),
            KeyCode::Char('P') => self.postpone(),
            KeyCode::Char('a') => self.prompt(PromptKind::Attach),
            KeyCode::Char('D') => {
                if compose.draft.attachments.pop().is_some() {
                    let _ = compose.rewrite();
                }
            }
            KeyCode::Char('q') if discarding => {
                if let Some(compose) = self.compose.take() {
                    compose.finish();
                }
                self.say("Discarded", false);
            }
            KeyCode::Char('q') => {
                compose.discarding = true;
                self.say("q again discards the Draft", false);
            }
            KeyCode::Char('?') => self.help(),
            _ => {}
        }
    }

    /// Sends the Draft, in the background: it's put away meanwhile, and comes back if it can't be sent.
    fn send(&mut self) {
        let Some(compose) = &self.compose else {
            return;
        };
        let Some(state) = self.accounts.get_mut(compose.account) else {
            return;
        };
        // At the domain of the address it's from.
        let from = compose
            .draft
            .from
            .as_ref()
            .or_else(|| state.sender.primary())
            .map_or("", |from| from.email.as_str());
        let id = muttrs_mail::compose::message_id(from);
        match compose.draft.build(&id) {
            Ok(built) => {
                let task = state.ask_as(
                    client::Request::Send {
                        built: Box::new(built),
                        answers: compose.answers.clone(),
                    },
                    Some(sending(&compose.draft)),
                );
                if let Some(compose) = self.compose.take() {
                    self.sending.push((task, compose));
                }
            }
            Err(error) => self.say(&format!("Not sent: {error}"), true),
        }
    }

    /// Takes what came of sending the Draft of `account`'s Task `task`: done with it, or it's shown again.
    fn sent(&mut self, account: usize, task: Task, sent: Result<(), String>) {
        let Some(at) = self
            .sending
            .iter()
            .position(|(sending, compose)| *sending == task && compose.account == account)
        else {
            return;
        };
        let (_, compose) = self.sending.remove(at);
        match sent {
            Ok(()) => {
                let subject = compose.draft.subject.trim();
                let text = if subject.is_empty() {
                    "Sent".to_owned()
                } else {
                    format!("Sent {subject}")
                };
                self.expunge_replaced(&compose);
                compose.finish();
                self.say(&text, false);
            }
            Err(why) => {
                self.say(&format!("Not sent, the Draft's back: {why}"), true);
                self.unsent.push(compose);
            }
        }
    }

    /// Shows again a Draft that couldn't be sent, once no other is shown, nor anything typed or chosen.
    fn bring_back_unsent(&mut self) {
        if self.compose.is_none()
            && self.prompt.is_none()
            && self.overlay.is_none()
            && self.sign_in.is_none()
            && !self.unsent.is_empty()
        {
            self.compose = Some(self.unsent.remove(0));
        }
    }

    /// Expunges the Postponed Draft `compose` was resumed from: it's sent, or kept again.
    fn expunge_replaced(&mut self, compose: &Compose) {
        if let (Some((mailbox, uid)), Some(state)) =
            (&compose.replaces, self.accounts.get_mut(compose.account))
        {
            state.ask(client::Request::Expunge {
                mailbox: mailbox.clone(),
                uids: vec![*uid],
            });
        }
    }

    /// Resumes the Postponed Draft the Pager shows, or the Index's cursor is on, in the Drafts Mailbox.
    fn resume(&mut self) {
        let in_drafts = self.index.as_ref().is_some_and(|index| {
            self.accounts
                .get(index.account)
                .and_then(|state| state.mailbox(&index.mailbox))
                .is_some_and(|mailbox| mailbox.role == Some(Role::Drafts))
        });
        if in_drafts {
            self.write(Writing::Resume);
        } else {
            self.say("e resumes a Postponed Draft, in the Drafts Mailbox", false);
        }
    }

    /// Saves the Draft to the Drafts Mailbox, to finish later.
    fn postpone(&mut self) {
        let Some(compose) = &self.compose else { return };
        let Some(state) = self.accounts.get_mut(compose.account) else {
            return;
        };
        // At the domain of the address it's from.
        let from = compose
            .draft
            .from
            .as_ref()
            .or_else(|| state.sender.primary())
            .map_or("", |from| from.email.as_str());
        let id = muttrs_mail::compose::message_id(from);
        match compose.draft.build(&id) {
            Ok(built) => {
                state.ask(client::Request::Append {
                    role: Role::Drafts,
                    message: built.kept,
                    draft: true,
                });
                if let Some(compose) = self.compose.take() {
                    self.expunge_replaced(&compose);
                    compose.finish();
                }
                self.say("Postponed to Drafts", false);
            }
            Err(error) => self.say(&format!("Not postponed: {error}"), true),
        }
    }

    /// Shows the Index of `account`'s Mailbox `name`.
    fn open_mailbox(&mut self, account: usize, name: &str) {
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        let Some(mailbox) = state.mailbox(name) else {
            return;
        };
        if !mailbox.selectable {
            self.say(
                &format!("{} only holds other Mailboxes", mailbox.label()),
                false,
            );
            return;
        }
        let mut index = Index::new(
            account,
            name.to_owned(),
            mailbox.label().to_owned(),
            mailbox.role,
            self.config.threads,
        )
        .hiding_repeated_subjects(self.config.hide_thread_subjects);
        // As last listed, until the server says what's changed: as it'll be once what's asked is done.
        if let Some(mut listed) = state.cache.envelopes(name) {
            state.tasks.apply(name, &mut listed.envelopes);
            index.set(listed.envelopes, listed.exists);
        }
        self.accounts[account].ask(client::Request::Open(name.to_owned()));
        self.sidebar
            .select(&self.accounts, &Row::Mailbox(account, name.to_owned()));
        self.searched = None;
        self.index = Some(index);
        self.focus = Focus::Index;
    }

    /// Fetches the Message at the Index's cursor, to show in the Pager. It's marked read at once when
    /// `pager_read_delay` is 0, or when it's fetched to write about; otherwise once it's shown for that long.
    fn open_current(&mut self) {
        let Some(index) = &self.index else { return };
        let Some(envelope) = index.current() else {
            return;
        };
        let opened = Opened {
            account: index.account,
            mailbox: index.mailbox.clone(),
            uid: envelope.uid,
        };
        let Some(state) = self.accounts.get_mut(opened.account) else {
            return;
        };
        state.ask(client::Request::Body {
            mailbox: opened.mailbox.clone(),
            uid: opened.uid,
        });
        // The Message shown before, if it waited to be marked read, stays Unread.
        self.reading = None;
        if self.config.pager_read_delay == 0 || self.write_when_fetched.is_some() {
            self.mark_seen(&opened);
        }
        self.opened = Some(opened);
        if self.pager.is_none() {
            self.say("Fetching the Message…", false);
        }
    }

    /// The Messages an action is for, by UID, and their Account and Mailbox: the one the Pager shows, or those
    /// selected in the Index, or the one at its cursor with the replies folded into it.
    fn targets(&self) -> Option<(usize, String, Vec<u32>)> {
        if self.pager.is_some() {
            let opened = self.opened.as_ref()?;
            return Some((opened.account, opened.mailbox.clone(), vec![opened.uid]));
        }
        let index = self.index.as_ref()?;
        let uids = index.targets();
        (!uids.is_empty()).then(|| (index.account, index.mailbox.clone(), uids))
    }

    /// Toggles `flag` on the Messages an action is for: on all, unless all have it already.
    fn toggle(&mut self, flag: Flag) {
        let Some((account, mailbox, uids)) = self.targets() else {
            return;
        };
        if flag == Flag::Seen {
            // Marked as the user says, rather than once it's been shown a while.
            self.reading = None;
        }
        let Some(index) = self.index.as_mut() else {
            return;
        };
        let has = |flags: &muttrs_mail::Flags| match flag {
            Flag::Seen => flags.seen,
            Flag::Flagged => flags.flagged,
            Flag::Answered => flags.answered,
        };
        let on = !uids
            .iter()
            .filter_map(|&uid| index.find(uid))
            .all(|envelope| has(&envelope.flags));
        let mut changed = 0;
        for &uid in &uids {
            let Some(envelope) = index.find_mut(uid) else {
                continue;
            };
            if has(&envelope.flags) != on {
                changed += 1;
            }
            tasks::set_flag(&mut envelope.flags, flag, on);
        }
        if let Some(state) = self.accounts.get_mut(account) {
            if flag == Flag::Seen
                && let Some(counts) = state.counts.get_mut(&mailbox)
            {
                counts.unread = if on {
                    counts.unread.saturating_sub(changed)
                } else {
                    counts.unread + changed
                };
            }
            state.ask(client::Request::Flag {
                mailbox,
                uids,
                flag,
                on,
            });
        }
    }

    /// Deletes (to Trash) or archives the Messages an action is for, going on to the next one.
    fn remove(&mut self, archive: bool) {
        let Some((account, mailbox, uids)) = self.targets() else {
            return;
        };
        // Deleted from Trash, they're deleted for good: there's no undoing that.
        let in_trash = self.accounts.get(account).is_some_and(|state| {
            state
                .mailbox(&mailbox)
                .is_some_and(|known| known.role == Some(Role::Trash))
        });
        if !in_trash {
            let went = if archive {
                client::Went::Archived
            } else {
                client::Went::Trash
            };
            self.remember_undo(account, &mailbox, &uids, went);
        }
        let Some(state) = self.accounts.get_mut(account) else {
            return;
        };
        // The worker knows where to: the Mailbox with the Role, by the server or the configuration. Without one it
        // says so, and lists the Mailbox again, which brings the Messages back.
        state.ask(if archive {
            client::Request::Archive {
                mailbox,
                uids: uids.clone(),
            }
        } else {
            client::Request::Delete {
                mailbox,
                uids: uids.clone(),
            }
        });
        self.taken_out(&uids);
    }

    /// Keeps, for `u` to undo, that `account`'s Messages `uids` of `mailbox` `went` elsewhere: those with a
    /// `Message-ID`, which find them there.
    fn remember_undo(&mut self, account: usize, mailbox: &str, uids: &[u32], went: client::Went) {
        let message_ids: Vec<String> = self
            .index
            .iter()
            .flat_map(|index| uids.iter().filter_map(|&uid| index.find(uid)))
            .filter_map(|envelope| envelope.message_id.clone())
            .collect();
        if message_ids.is_empty() {
            return;
        }
        if self.undo.len() == UNDOES {
            self.undo.remove(0);
        }
        self.undo.push(Undo {
            account,
            went,
            from: mailbox.to_owned(),
            message_ids,
        });
    }

    /// Moves the Messages last moved, archived or deleted to Trash back where they were.
    fn undo(&mut self) {
        let Some(undo) = self.undo.pop() else {
            self.say("Nothing to undo", false);
            return;
        };
        if let Some(state) = self.accounts.get_mut(undo.account) {
            state.ask(client::Request::MoveBack {
                went: undo.went,
                to: undo.from,
                message_ids: undo.message_ids,
            });
        }
    }

    /// Takes the Messages `uids` out of the Index, as they've gone elsewhere, going on to the next one in the Pager.
    fn taken_out(&mut self, uids: &[u32]) {
        if let Some(index) = &mut self.index {
            index.remove(uids);
        }
        if self.pager.is_some() {
            self.close_pager();
            self.open_current();
        }
    }

    /// Lists the Account's other Mailboxes, to move the Messages an action is for to one.
    fn choose_mailbox(&mut self) {
        let Some((account, mailbox, uids)) = self.targets() else {
            return;
        };
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        let rows: Vec<Line<'static>> = self
            .moving_to(account, &mailbox)
            .iter()
            .map(|other| {
                Line::from(vec![
                    Span::styled(sidebar::icon(&self.icons, other.role), self.theme.dim),
                    Span::raw(other.path.join("/")),
                ])
            })
            .collect();
        if rows.is_empty() {
            self.say(&format!("{} has no other Mailbox", state.name), false);
            return;
        }
        let what = match uids.len() {
            1 => "Move to".to_owned(),
            count => format!("Move {count} to"),
        };
        self.overlay = Some(Overlay::new(overlay::Kind::Mailboxes, what, rows));
    }

    /// Lists the Account's Labels, each marked as the Messages an action is for have it, to add or take off.
    fn choose_labels(&mut self) {
        let Some((account, _, uids)) = self.targets() else {
            return;
        };
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        if state.labels.is_empty() {
            let why = if state.connection == Connection::Online {
                format!("{} has no Labels", state.name)
            } else {
                format!("{}'s Labels wait for the server", state.name)
            };
            self.say(&why, false);
            return;
        }
        let rows = (0..state.labels.len())
            .map(|label| self.label_row(account, &uids, label))
            .collect();
        let title = match uids.len() {
            1 => "Labels".to_owned(),
            count => format!("Labels of {count}"),
        };
        self.overlay = Some(Overlay::new(overlay::Kind::Labels, title, rows));
    }

    /// How many of `uids`, of the Index, have `account`'s `label`th Label, of how many the Index lists.
    fn labelled(&self, account: usize, uids: &[u32], label: usize) -> (usize, usize) {
        let (Some(index), Some(label)) = (
            self.index.as_ref(),
            self.accounts
                .get(account)
                .and_then(|state| state.labels.get(label)),
        ) else {
            return (0, 0);
        };
        let listed: Vec<&muttrs_mail::Envelope> =
            uids.iter().filter_map(|&uid| index.find(uid)).collect();
        let have = listed
            .iter()
            .filter(|envelope| envelope.labels.contains(&label.name))
            .count();
        (have, listed.len())
    }

    /// The row of the Labels' list for `account`'s `label`th: marked if all of `uids` have it, half if some do.
    fn label_row(&self, account: usize, uids: &[u32], label: usize) -> Line<'static> {
        let name = self.accounts[account].labels[label].name.clone();
        let mark = match self.labelled(account, uids, label) {
            (0, _) => "  ",
            (have, all) if have == all => "✓ ",
            _ => "– ",
        };
        Line::from(vec![Span::styled(mark, self.theme.accent), Span::raw(name)])
    }

    /// Adds the `chosen`th of the Account's Labels to the Messages an action is for: to all of them, unless all have it
    /// already, when it's taken off them.
    fn toggle_label(&mut self, chosen: usize) {
        let Some((account, mailbox, uids)) = self.targets() else {
            return;
        };
        let Some(label) = self
            .accounts
            .get(account)
            .and_then(|state| state.labels.get(chosen))
            .cloned()
        else {
            return;
        };
        let (have, all) = self.labelled(account, &uids, chosen);
        let on = have < all || all == 0;
        // As it will be, until the server says.
        if let Some(index) = self.index.as_mut() {
            for &uid in &uids {
                if let Some(envelope) = index.find_mut(uid) {
                    tasks::set_label(&mut envelope.labels, &label.name, on);
                }
            }
        }
        let row = self.label_row(account, &uids, chosen);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.set_row(chosen, row);
        }
        self.accounts[account].ask(client::Request::Label {
            mailbox,
            uids,
            label: label.mailbox,
            on,
        });
    }

    /// The Mailboxes of `account` Messages of `mailbox` can be moved to, in the sidebar's order.
    fn moving_to(&self, account: usize, mailbox: &str) -> Vec<muttrs_mail::Mailbox> {
        sidebar::rows(&self.accounts)
            .into_iter()
            .filter_map(|row| match row {
                Row::Mailbox(at, name) if at == account && name != mailbox => {
                    self.accounts[at].mailbox(&name).cloned()
                }
                _ => None,
            })
            .filter(|other| other.selectable)
            .collect()
    }

    /// Moves the Messages an action is for to the `chosen`th of the Mailboxes they can go to.
    fn move_to(&mut self, chosen: usize) {
        let Some((account, mailbox, uids)) = self.targets() else {
            return;
        };
        let Some(target) = self.moving_to(account, &mailbox).into_iter().nth(chosen) else {
            return;
        };
        self.remember_undo(
            account,
            &mailbox,
            &uids,
            client::Went::Mailbox(target.name.clone()),
        );
        if let Some(state) = self.accounts.get_mut(account) {
            state.ask(client::Request::Move {
                mailbox,
                uids: uids.clone(),
                to: target.name,
            });
        }
        self.taken_out(&uids);
    }

    fn on_terminal(&mut self, event: &TerminalEvent) {
        match event {
            TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => self.on_key(*key),
            TerminalEvent::Mouse(mouse) => self.on_mouse(*mouse),
            TerminalEvent::Resize(..) => {
                let cell = cell_size();
                if cell != self.pictures.cell() {
                    self.pictures.set_cell(cell);
                    if let Some(pager) = &mut self.pager {
                        pager.cell_changed();
                    }
                }
            }
            _ => {}
        }
    }

    /// The wheel scrolls what it's over; a click chooses a row, and a second click on it opens it.
    fn on_mouse(&mut self, mouse: MouseEvent) {
        if self.overlay.is_some() || self.prompt.is_some() || self.sign_in.is_some() {
            return;
        }
        let lines = match mouse.kind {
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::Down(MouseButton::Left) => 0,
            _ => return,
        };
        if let Some(compose) = &mut self.compose {
            compose.scroll_by(lines);
            return;
        }
        let (sidebar, index_area) = self.drawn;
        let inside = |area: Rect| {
            mouse.column >= area.x
                && mouse.column < area.x + area.width
                && mouse.row >= area.y
                && mouse.row < area.y + area.height
        };
        let offset = |area: Rect| usize::from(mouse.row.saturating_sub(area.y));
        if self.pager.is_some() {
            // A click on the mini-index opens the Message clicked; the wheel scrolls the Pager wherever it is.
            if lines == 0 && inside(index_area) {
                if let Some(index) = &mut self.index
                    && index.click_around_cursor(offset(index_area))
                {
                    self.more_if_wanted();
                    self.open_current();
                }
            } else if let Some(pager) = &mut self.pager {
                pager.scroll_by(lines);
            }
            return;
        }
        if inside(sidebar) {
            if lines != 0 {
                self.sidebar.move_by(&self.accounts, lines);
            } else if let Some(row) = self.sidebar.click(&self.accounts, offset(sidebar)) {
                self.focus = Focus::Sidebar;
                if let Row::Mailbox(account, name) = row {
                    self.open_mailbox(account, &name);
                }
            }
        } else if inside(index_area) {
            let Some(index) = &mut self.index else { return };
            if lines != 0 {
                index.move_by(lines);
            } else {
                self.focus = Focus::Index;
                if !index.click(offset(index_area)) && index.is_cursor_at(offset(index_area)) {
                    self.open_current();
                }
            }
            self.more_if_wanted();
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        self.notice = None;
        // A key stays: quitting once the Tasks are done is asked again, if it's to be.
        self.quitting = false;
        if self.sign_in.is_some() {
            self.sign_in_key(key);
            return;
        }
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.prompt.is_none()
        {
            self.ask_to_quit();
            return;
        }
        if self.prompt.is_some() {
            self.prompt_key(key);
            return;
        }
        if let Some(overlay) = &mut self.overlay {
            match overlay.key(key) {
                Outcome::Stay => {}
                Outcome::Close => self.overlay = None,
                Outcome::Chosen(index, verb) => {
                    let kind = overlay.kind();
                    self.chosen(kind, index, verb);
                }
            }
            return;
        }
        if self.compose.is_some() {
            self.compose_key(key);
        } else if self.pager.is_some() {
            self.pager_key(key);
        } else {
            self.main_key(key);
        }
    }

    /// Keys for the sidebar and the Index.
    fn main_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => {
                self.ask_to_quit();
                return;
            }
            KeyCode::Char(':') => {
                self.prompt(PromptKind::Command);
                return;
            }
            KeyCode::Char('?') => {
                self.help();
                return;
            }
            KeyCode::Char('\\') => {
                self.sidebar_shown = !self.sidebar_shown;
                self.focus = if self.sidebar_shown {
                    Focus::Sidebar
                } else {
                    Focus::Index
                };
                return;
            }
            KeyCode::Tab | KeyCode::BackTab if self.sidebar_shown => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Index,
                    Focus::Index => Focus::Sidebar,
                };
                return;
            }
            KeyCode::Char('$') => {
                self.refresh();
                return;
            }
            KeyCode::Char('r') if control => {
                self.refresh();
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Sidebar => self.sidebar_key(key),
            Focus::Index => self.index_key(key),
        }
        self.more_if_wanted();
    }

    /// Asks for older Envelopes, if the Index's cursor nears the end of those it lists.
    fn more_if_wanted(&mut self) {
        let Some(index) = &mut self.index else { return };
        if index.wants_more()
            && let Some(state) = self.accounts.get_mut(index.account)
        {
            state.ask(client::Request::More(index.mailbox.clone()));
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.sidebar.move_by(&self.accounts, 1),
            KeyCode::Char('k') | KeyCode::Up => self.sidebar.move_by(&self.accounts, -1),
            KeyCode::Char('g') | KeyCode::Home => self.sidebar.top(),
            KeyCode::Char('G') | KeyCode::End => self.sidebar.bottom(&self.accounts),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Esc => self.focus = Focus::Index,
            KeyCode::Enter => match self.sidebar.current(&self.accounts) {
                Some(Row::Mailbox(account, name)) => self.open_mailbox(account, &name),
                Some(Row::Account(account)) => {
                    if let Some(state) = self.accounts.get_mut(account) {
                        state.ask(client::Request::Refresh);
                    }
                }
                None => {}
            },
            _ => {}
        }
    }

    fn index_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(key.code, KeyCode::Char('h') | KeyCode::Left) && self.sidebar_shown {
            self.focus = Focus::Sidebar;
            return;
        }
        let Some(index) = &mut self.index else { return };
        if self.pending.take() == Some('z') {
            match key.code {
                KeyCode::Char('a' | 'A' | 'c' | 'o') => index.toggle_fold(),
                KeyCode::Char('M') => index.fold_all(true),
                KeyCode::Char('R') => index.fold_all(false),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('z') => self.pending = Some('z'),
            KeyCode::Char('T') => {
                let threaded = index.toggle_threads();
                self.say(
                    if threaded {
                        "In threads"
                    } else {
                        "Newest first"
                    },
                    false,
                );
            }
            KeyCode::Char('j') | KeyCode::Down => index.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => index.move_by(-1),
            KeyCode::Char('d') if control => index.move_pages(0.5),
            KeyCode::Char('u') if control => index.move_pages(-0.5),
            KeyCode::Char('f') if control => index.move_pages(1.0),
            KeyCode::Char('b') if control => index.move_pages(-1.0),
            KeyCode::PageDown => index.move_pages(1.0),
            KeyCode::PageUp => index.move_pages(-1.0),
            KeyCode::Char('g') | KeyCode::Home => index.top(),
            KeyCode::Char('G') | KeyCode::End => index.bottom(),
            KeyCode::Char('n') => index.next_match(true),
            KeyCode::Char('N') => index.next_match(false),
            KeyCode::Esc => {
                // Back from what a search on the server found, once there's no search of the Index, nor a
                // Selection, to clear.
                if !index.clear_search()
                    && !index.clear_selected()
                    && index.found.is_some()
                    && let Some(searched) = self.searched.take()
                {
                    self.index = Some(searched);
                }
            }
            KeyCode::Char('/') => self.prompt(PromptKind::Search),
            KeyCode::Char('S') => self.prompt(PromptKind::Server),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_current(),
            KeyCode::Char(' ') => index.toggle_selected(),
            KeyCode::Char('F') => self.toggle(Flag::Flagged),
            KeyCode::Char('U') => self.toggle(Flag::Seen),
            KeyCode::Char('Y') => self.choose_labels(),
            KeyCode::Char('d') => self.remove(false),
            KeyCode::Char('a') => self.remove(true),
            KeyCode::Char('s') => self.choose_mailbox(),
            KeyCode::Char('u') if !control => self.undo(),
            KeyCode::Char('e') => self.resume(),
            KeyCode::Char('m') => self.write(Writing::New),
            KeyCode::Char('r') => self.write(Writing::Reply),
            KeyCode::Char('R') => self.write(Writing::ReplyAll),
            KeyCode::Char('f') => self.write(Writing::Forward),
            _ => {}
        }
    }

    /// Quits once the Accounts' Tasks are done: at once if none's left, else once the last is finished, unless a key
    /// is pressed first.
    fn ask_to_quit(&mut self) {
        self.quitting = true;
        if self.quit_if_done() {
            return;
        }
        let left: usize = self.accounts.iter().map(|state| state.tasks.len()).sum();
        let tasks = if left == 1 {
            "the Task left is".to_owned()
        } else {
            format!("the {left} Tasks left are")
        };
        self.say(
            &format!("Quitting once {tasks} done: a key stays, :q! quits now"),
            false,
        );
    }

    /// Quits if quitting was asked and the Accounts' Tasks are done; whether it quits.
    fn quit_if_done(&mut self) -> bool {
        if self.quitting && self.accounts.iter().all(|state| state.tasks.is_empty()) {
            self.quit = true;
        }
        self.quit
    }

    fn prompt(&mut self, kind: PromptKind) {
        self.prompt = Some(Prompt::new(kind, String::new()));
    }

    /// Forgets what's kept of the open Mailbox, and lists it anew.
    fn resync(&mut self) {
        let Some(index) = &self.index else {
            self.say("No Mailbox is open", true);
            return;
        };
        let mailbox = index.mailbox.clone();
        if let Some(state) = self.accounts.get_mut(index.account) {
            state.ask(client::Request::Resync(mailbox));
        }
    }

    /// Asks every Account what's new, connecting again any that's offline.
    fn refresh(&mut self) {
        for account in &mut self.accounts {
            account.ask(client::Request::Refresh);
        }
    }

    fn pager_key(&mut self, key: KeyEvent) {
        let Some(pager) = &mut self.pager else { return };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => pager.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => pager.scroll_by(-1),
            KeyCode::Char('e') if control => pager.scroll_by(1),
            KeyCode::Char('y') if control => pager.scroll_by(-1),
            KeyCode::Char('d') if control => pager.scroll_pages(0.5),
            KeyCode::Char('u') if control => pager.scroll_pages(-0.5),
            KeyCode::Char('f') if control => pager.scroll_pages(1.0),
            KeyCode::Char('b') if control => pager.scroll_pages(-1.0),
            KeyCode::Char(' ') | KeyCode::PageDown => pager.scroll_pages(1.0),
            KeyCode::Char('b') | KeyCode::PageUp => pager.scroll_pages(-1.0),
            KeyCode::Char('g') | KeyCode::Home => pager.top(),
            KeyCode::Char('G') | KeyCode::End => pager.bottom(),
            KeyCode::Char('n') => pager.next_match(true),
            KeyCode::Char('N') => pager.next_match(false),
            KeyCode::Char('$') => self.refresh(),
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::new(PromptKind::Search, String::new()));
            }
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()));
            }
            KeyCode::Char('T') => {
                if !pager.toggle_body() {
                    self.say("This Message has one body only", false);
                }
            }
            KeyCode::Char('I') => {
                let request = pager.allow_remote();
                if request == Request::None {
                    self.say("No Remote Content to load", false);
                }
                self.fulfil(request);
            }
            KeyCode::Char('o') => self.links(),
            KeyCode::Char('v') => self.attachments(),
            KeyCode::Char('H') => self.toggle_heatmap(),
            KeyCode::Char('?') => self.help(),
            KeyCode::Esc => {
                if !pager.clear_search() {
                    self.close_pager();
                }
            }
            KeyCode::Char('q') => self.close_pager(),
            _ if self.standalone => {}
            KeyCode::Char('J') => self.step(true),
            KeyCode::Char('K') => self.step(false),
            KeyCode::Char('F') => self.toggle(Flag::Flagged),
            KeyCode::Char('U') => self.toggle(Flag::Seen),
            KeyCode::Char('Y') => self.choose_labels(),
            KeyCode::Char('d') => self.remove(false),
            KeyCode::Char('a') => self.remove(true),
            KeyCode::Char('s') => self.choose_mailbox(),
            KeyCode::Char('u') if !control => self.undo(),
            KeyCode::Char('e') => self.resume(),
            KeyCode::Char('m') => self.write(Writing::New),
            KeyCode::Char('r') => self.write(Writing::Reply),
            KeyCode::Char('R') => self.write(Writing::ReplyAll),
            KeyCode::Char('f') => self.write(Writing::Forward),
            _ => {}
        }
    }

    /// Opens the next Message, older, or the previous one, newer.
    fn step(&mut self, older: bool) {
        let Some(index) = &mut self.index else { return };
        if index.step(older) {
            self.more_if_wanted();
            self.open_current();
        } else {
            self.say(
                if older {
                    "No older Message"
                } else {
                    "No newer Message"
                },
                false,
            );
        }
    }

    fn close_pager(&mut self) {
        self.pager = None;
        self.opened = None;
        self.reading = None;
        self.pictures.keep(None, |_| false);
        // Standing alone, as `muttrs view` opens it, the Pager is all there is.
        if self.standalone {
            self.quit = true;
        }
    }

    fn prompt_key(&mut self, key: KeyEvent) {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => return self.recall(true, true),
            KeyCode::Down => return self.recall(false, true),
            KeyCode::Char('p') if control => return self.recall(true, false),
            KeyCode::Char('n') if control => return self.recall(false, false),
            _ => {}
        }
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        prompt.recalling = None;
        if !matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            prompt.completing = None;
        }
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.writing_to = None;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.prompt = None;
            }
            KeyCode::Backspace => {
                if prompt.text.pop().is_none() {
                    self.prompt = None;
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.text.clear();
            }
            KeyCode::Tab | KeyCode::BackTab
                if matches!(prompt.kind, PromptKind::To | PromptKind::Command) =>
            {
                self.complete(key.code == KeyCode::Tab);
            }
            KeyCode::Char(character) => prompt.text.push(character),
            KeyCode::Enter => {
                let Prompt { kind, text, .. } = self.prompt.take().expect("a prompt");
                if let Some(history) = kind.history() {
                    // As it's run: a search of what's listed or shown takes it as typed, spaces too.
                    let entry = if kind == PromptKind::Search {
                        text.as_str()
                    } else {
                        text.trim()
                    };
                    self.history.add(history, entry);
                }
                match kind {
                    PromptKind::Search if !text.is_empty() => {
                        if let Some(pager) = &mut self.pager {
                            pager.search_for(text);
                        } else if let Some(index) = &mut self.index {
                            index.search_for(text);
                        }
                    }
                    PromptKind::Search => {}
                    PromptKind::Command => self.command(text.trim()),
                    PromptKind::Attach => self.attach(text.trim()),
                    PromptKind::Server => self.search_server(text.trim()),
                    PromptKind::To => self.addressed(text),
                    PromptKind::Subject => self.subject(&text),
                }
            }
            _ => {}
        }
    }

    /// Shows what was typed at the prompt before, from its History: the entry older than the one shown, or newer,
    /// of those that start with what was typed if `matching` (as `Up` and `Down` do), else of all of them.
    fn recall(&mut self, older: bool, matching: bool) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        let Some(kind) = prompt.kind.history() else {
            return;
        };
        prompt.completing = None;
        let recalling = prompt
            .recalling
            .get_or_insert_with(|| Recall::new(prompt.text.clone()));
        if let Some(entry) = recalling.step(self.history.entries(kind), older, matching) {
            prompt.text = entry.to_owned();
        }
    }

    /// Completes what's typed last in the prompt, as its kind does: an address; or a command, then a filter's word.
    /// Pressed again, it goes through the other ways, forward or back.
    fn complete(&mut self, forward: bool) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        if let Some(completing) = &mut prompt.completing {
            let count = completing.candidates.len();
            completing.at = if forward {
                (completing.at + 1) % count
            } else {
                (completing.at + count - 1) % count
            };
            prompt.text = format!(
                "{}{}",
                completing.before, completing.candidates[completing.at]
            );
            return;
        }
        let (before, candidates) = if prompt.kind == PromptKind::To {
            match prompt.text.rsplit_once(',') {
                Some((before, fragment)) => {
                    (format!("{before}, "), self.addresses.complete(fragment))
                }
                None => (String::new(), self.addresses.complete(&prompt.text)),
            }
        } else {
            let listed = self.index.as_ref().map_or(&[][..], Index::envelopes);
            (String::new(), command_completions(&prompt.text, listed))
        };
        if let Some(first) = candidates.first() {
            prompt.text = format!("{before}{first}");
            prompt.completing = Some(Completing {
                before,
                candidates,
                at: 0,
            });
        }
    }

    /// Takes whom a new Message is to, and asks its subject.
    fn addressed(&mut self, to: String) {
        match muttrs_mail::compose::parse_addresses(to.trim().trim_end_matches(',')) {
            Ok(addresses) => {
                self.writing_to = Some(addresses);
                self.prompt = Some(Prompt::new(PromptKind::Subject, String::new()));
            }
            Err(error) => {
                self.say(&error.to_string(), true);
                self.prompt = Some(Prompt::new(PromptKind::To, to));
            }
        }
    }

    /// Takes a new Message's subject, and opens it in the editor.
    fn subject(&mut self, subject: &str) {
        let to = self.writing_to.take().unwrap_or_default();
        let account = self.index.as_ref().map_or(0, |index| index.account);
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        let mut draft = muttrs_mail::compose::Draft::new(&state.sender);
        draft.to = to;
        subject.trim().clone_into(&mut draft.subject);
        self.start(account, draft, None);
    }

    fn command(&mut self, command: &str) {
        let (name, argument) = command
            .split_once(' ')
            .map_or((command, ""), |(name, argument)| (name, argument.trim()));
        match name {
            "" => {}
            "q" | "quit" | "qa" => self.ask_to_quit(),
            "q!" | "qa!" => self.quit = true,
            "help" | "h" => self.help(),
            "refresh" | "sync" => self.refresh(),
            "refresh!" | "sync!" => self.resync(),
            "login" => self.login(argument),
            "cd" | "open" if !argument.is_empty() => self.change_mailbox(argument),
            "select" => self.select_by(true, argument),
            "unselect" => self.select_by(false, argument),
            _ => self.say(&format!("Not a command: {command}"), true),
        }
    }

    /// Selects, or unselects, the Messages the Index lists that `filter` picks: all of them, without one.
    fn select_by(&mut self, on: bool, filter: &str) {
        let parsed = match filter.parse::<Filter>() {
            Ok(parsed) => parsed,
            Err(error) => {
                self.say(&error.to_string(), true);
                return;
            }
        };
        let Some(index) = &mut self.index else { return };
        let (picked, changed) = index.select_where(on, |envelope| parsed.matches(envelope));
        let done = if on { "selected" } else { "unselected" };
        let text = match (picked, changed) {
            (0, _) if filter.is_empty() => "No Message listed".to_owned(),
            (0, _) => format!("No Message listed matches {filter}"),
            (_, 0) if on => "All of those are selected already".to_owned(),
            (_, 0) => "None of those is selected".to_owned(),
            (_, 1) => format!("1 Message {done}"),
            (_, changed) => format!("{changed} Messages {done}"),
        };
        self.say(&text, false);
    }

    /// Opens the Mailbox called `wanted` (by its name for people, or on the server), of the Account shown first.
    fn change_mailbox(&mut self, wanted: &str) {
        let account = self.index.as_ref().map_or(0, |index| index.account);
        let found = self.accounts.get(account).and_then(|state| {
            state
                .mailboxes
                .iter()
                .find(|mailbox| {
                    mailbox.label().eq_ignore_ascii_case(wanted)
                        || mailbox.name.eq_ignore_ascii_case(wanted)
                        || mailbox.path.join("/").eq_ignore_ascii_case(wanted)
                })
                .map(|mailbox| mailbox.name.clone())
        });
        match found {
            Some(name) => {
                self.close_pager();
                self.open_mailbox(account, &name);
            }
            None => self.say(&format!("No Mailbox called {wanted}"), true),
        }
    }

    /// Searches the open Mailbox on the server for `query`.
    fn search_server(&mut self, query: &str) {
        let Some(index) = &self.index else { return };
        if query.is_empty() {
            return;
        }
        // The status line says it's searching.
        if let Some(state) = self.accounts.get_mut(index.account) {
            state.ask(client::Request::Search {
                mailbox: index.mailbox.clone(),
                query: query.to_owned(),
            });
        }
    }

    /// Attaches the file at `path` to the Draft.
    fn attach(&mut self, path: &str) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        if path.is_empty() {
            return;
        }
        let path = xdg::expand(path);
        if !path.is_file() {
            self.say(&format!("{} isn't a file", path.display()), true);
            return;
        }
        compose
            .draft
            .attachments
            .push(muttrs_mail::compose::Attachment::file(path));
        let _ = compose.rewrite();
    }

    /// Signs in again to the Account called `name`, or else the Index's, in the browser.
    fn login(&mut self, name: &str) {
        let index = if name.is_empty() {
            self.index.as_ref().map_or(0, |index| index.account)
        } else if let Some(index) = self.accounts.iter().position(|state| state.name == name) {
            index
        } else {
            self.say(&format!("No account called {name}"), true);
            return;
        };
        let Some(state) = self.accounts.get(index) else {
            self.say("There are no accounts", true);
            return;
        };
        let started = login::account(&self.config, &state.name)
            .and_then(|account| SignIn::start(index, account, self.events.clone()));
        match started {
            Ok(sign_in) => self.sign_in = Some(sign_in),
            Err(error) => self.say(&format!("{error:#}"), true),
        }
    }

    /// Whether the Account called `name` signs in with OAuth2, and so can sign in again with `:login`.
    fn signs_in_with_oauth2(&self, name: &str) -> bool {
        login::account(&self.config, name)
            .is_ok_and(|account| matches!(account.auth, muttrs_mail::account::Auth::OAuth2(_)))
    }

    fn sign_in_key(&mut self, key: KeyEvent) {
        let Some(sign_in) = &mut self.sign_in else {
            return;
        };
        match sign_in.key(key) {
            sign_in::Outcome::Stay => {}
            sign_in::Outcome::Cancel => self.sign_in = None,
            sign_in::Outcome::Copy => {
                copy(sign_in.url());
                self.say("Copied the address of the page to sign in at", false);
            }
            sign_in::Outcome::Open => {
                if let Err(error) = open::that_detached(sign_in.url()) {
                    self.say(&format!("Couldn't open the browser: {error}"), true);
                }
            }
        }
    }

    /// Takes in how the sign-in `id` goes, if it's the one under way.
    fn signing_in(&mut self, id: u64, progress: sign_in::Progress) {
        let Some(sign_in) = self.sign_in.as_mut().filter(|sign_in| sign_in.is(id)) else {
            return;
        };
        let signed_in = match progress {
            sign_in::Progress::Rejected(why) => {
                sign_in.reject(why);
                return;
            }
            sign_in::Progress::Done(signed_in) => signed_in,
        };
        let account = sign_in.account;
        self.sign_in = None;
        let Some(state) = self.accounts.get_mut(account) else {
            return;
        };
        let text = match &signed_in {
            Ok(()) => {
                state.ask(client::Request::Refresh);
                format!("{}: signed in", state.name)
            }
            Err(why) => format!("{}: signing in failed: {why}", state.name),
        };
        self.say(&text, signed_in.is_err());
    }

    fn say(&mut self, text: &str, error: bool) {
        self.notice = Some(Notice {
            text: text.to_owned(),
            error,
        });
    }

    fn links(&mut self) {
        let Some(pager) = &self.pager else { return };
        let links = pager.links();
        if links.is_empty() {
            self.say("No Links in this Message", false);
            return;
        }
        let width = links.len().to_string().len();
        let rows = links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let mut spans = vec![Span::styled(
                    format!("{:>width$} ", index + 1),
                    self.theme.keys,
                )];
                let text = truncate(&link.text, 40);
                if text != link.url {
                    spans.push(Span::raw(format!("{text}  ")));
                }
                spans.push(Span::styled(truncate(&link.url, 80), self.theme.body.link));
                Line::from(spans)
            })
            .collect();
        self.overlay = Some(Overlay::new(overlay::Kind::Links, "Links", rows));
    }

    fn attachments(&mut self) {
        let Some(pager) = &self.pager else { return };
        let attachments = pager.attachments();
        if attachments.is_empty() {
            self.say("No Attachments in this Message", false);
            return;
        }
        let rows = attachments
            .iter()
            .map(|part| {
                Line::from(vec![
                    Span::styled(self.icons.attachment, self.theme.attachment),
                    Span::raw(part.file_name()),
                    Span::styled(
                        format!("  {} · {}", part.mime_type, pager::size(part.size)),
                        self.theme.dim,
                    ),
                ])
            })
            .collect();
        self.overlay = Some(Overlay::new(
            overlay::Kind::Attachments,
            "Attachments",
            rows,
        ));
    }

    fn help(&mut self) {
        self.overlay = Some(overlay::help(
            &self.theme,
            &[
                (
                    "Reading",
                    &[
                        ("j k ↓ ↑", "scroll a line"),
                        ("Space b", "a page down, up"),
                        ("Ctrl-d Ctrl-u", "half a page down, up"),
                        ("g G", "top, bottom"),
                        ("/ n N", "search, next match, previous"),
                        ("T", "HTML or plain text"),
                        ("I", "load Remote Content"),
                        ("H", "voight's Heatmap of the prose"),
                    ],
                ),
                (
                    "Parts",
                    &[
                        ("o", "Links: open or copy"),
                        ("v", "Attachments: open or save"),
                    ],
                ),
                (
                    "Mailboxes",
                    &[
                        ("j k g G", "move"),
                        ("Enter", "open"),
                        ("/ n N", "search senders and subjects"),
                        ("S", "search the server (Gmail's syntax on Gmail)"),
                        ("T", "threads, or newest first"),
                        ("za zM zR", "fold the thread, all, none"),
                        ("J K", "in the Pager: the next Message, the previous"),
                        ("Space", "select, or not, and move down"),
                        ("F U", "flag, mark Unread or read: the selected, or this"),
                        ("Y", "add or take off Labels: the selected, or this"),
                        ("d a s", "delete (to Trash), archive, move to a Mailbox"),
                        ("u", "undo the last of those: move them back"),
                        ("Tab h l", "the sidebar, the Index"),
                        ("\\", "show or hide the sidebar"),
                        (":cd NAME", "open a Mailbox by name"),
                        (
                            ":select F",
                            "select what filter F picks: from:ann is:unread label:work …",
                        ),
                        (":unselect F", "unselect what F picks; without F, all"),
                    ],
                ),
                (
                    "Writing",
                    &[
                        ("m", "a new Message"),
                        ("r R f", "reply, reply to all, forward"),
                        ("y e", "with a Draft: send, edit again"),
                        ("a D", "attach a file, detach the last"),
                        ("P q q", "postpone to Drafts, discard"),
                        ("e", "in Drafts: resume a Postponed Draft"),
                    ],
                ),
                (
                    "Anywhere",
                    &[
                        ("?", "these keys"),
                        (":q q", "quit once what's asked is done, or close the Pager"),
                        (":q!", "quit now"),
                        ("$ :refresh", "ask for what's new"),
                        (
                            ":sync!",
                            "forget what's kept of the open Mailbox, and list it anew",
                        ),
                        (":login NAME", "sign in to an Account again, in the browser"),
                        (
                            "↑ ↓",
                            "after : / or S, what was typed before, starting the same",
                        ),
                        (
                            "Ctrl-p Ctrl-n",
                            "after : / or S, what was typed before, all of it",
                        ),
                    ],
                ),
            ],
        ));
    }

    fn chosen(&mut self, kind: overlay::Kind, index: usize, verb: Verb) {
        // Moving and labelling need no Pager: what's moved or labelled is in the Index.
        if kind == overlay::Kind::Mailboxes {
            self.overlay = None;
            self.move_to(index);
            return;
        }
        if kind == overlay::Kind::Labels {
            self.toggle_label(index);
            return;
        }
        let Some(pager) = &self.pager else { return };
        match kind {
            overlay::Kind::Links => {
                let Some(link) = pager.links().get(index).cloned() else {
                    return;
                };
                match verb {
                    Verb::Copy => {
                        copy(&link.url);
                        self.say(&format!("Copied {}", link.url), false);
                    }
                    _ => match open::that_detached(&link.url) {
                        Ok(()) => self.say(&format!("Opened {}", link.url), false),
                        Err(error) => {
                            self.say(&format!("Couldn't open {}: {error}", link.url), true);
                        }
                    },
                }
            }
            overlay::Kind::Attachments => {
                let Some(part) = pager.attachments().get(index).map(|part| (*part).clone()) else {
                    return;
                };
                let bytes = pager.message().bytes(&part);
                let outcome = match verb {
                    Verb::Save => save(&self.config.download_dir(), &part.file_name(), &bytes)
                        .map(|path| format!("Saved {}", path.display())),
                    _ => open_attachment(pager.id(), &part.file_name(), &bytes)
                        .map(|path| format!("Opened {}", path.display())),
                };
                match outcome {
                    Ok(said) => self.say(&said, false),
                    Err(error) => self.say(&format!("{error:#}"), true),
                }
                self.overlay = None;
            }
            overlay::Kind::Mailboxes | overlay::Kind::Labels | overlay::Kind::Help => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        if area.height < 2 {
            return;
        }
        let body = Rect {
            height: area.height - 1,
            ..area
        };
        let status = Rect {
            y: area.y + area.height - 1,
            height: 1,
            ..area
        };
        let buffer = frame.buffer_mut();
        if let Some(compose) = &mut self.compose {
            compose.draw(
                body,
                buffer,
                &self.theme,
                &self.icons,
                self.config.pager_width,
            );
        } else if self.pager.is_some() {
            let heat = self.heat();
            let area = self.draw_mini_index(body, buffer);
            let Some(pager) = &mut self.pager else { return };
            pager.shade(heat);
            let mut cx = pager::Cx {
                theme: &self.theme,
                icons: &self.icons,
                pictures: &mut self.pictures,
                pager_width: self.config.pager_width,
                max_picture_rows: self.config.max_picture_rows,
                pixel_ratio: self.config.pixel_ratio,
            };
            pager.draw(area, buffer, &mut cx);
        } else if !self.standalone {
            self.draw_main(body, buffer);
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.draw(body, buffer, &self.theme);
        }
        if let Some(sign_in) = &self.sign_in {
            sign_in.draw(body, frame, &self.theme);
        }
        self.draw_status(status, frame);
    }

    /// The Index's rows around the Message the Pager shows, above it, then the Index's status line, as mutt's
    /// mini-index (`pager_index_lines`): what's left of `body` is the Pager's.
    fn draw_mini_index(&mut self, body: Rect, buffer: &mut ratatui::buffer::Buffer) -> Rect {
        // The mouse finds nothing but the Pager, unless the mini-index is drawn.
        self.drawn = (Rect::default(), Rect::default());
        let rows = match &self.index {
            Some(index) if !self.standalone => index.len(),
            _ => 0,
        };
        let Some((rows_area, status_area, pager_area)) =
            mini_index(body, self.config.pager_index_lines, rows)
        else {
            return body;
        };
        let status = self.index_status();
        if let Some(index) = &mut self.index {
            index.draw_around_cursor(
                rows_area,
                buffer,
                &index::Draw {
                    theme: &self.theme,
                    icons: &self.icons,
                    focused: false,
                },
            );
        }
        buffer.set_style(status_area, self.theme.status);
        buffer.set_line(
            status_area.x,
            status_area.y,
            &Line::raw(status),
            status_area.width,
        );
        self.drawn.1 = rows_area;
        pager_area
    }

    /// The sidebar, then the Index.
    fn draw_main(&mut self, area: Rect, buffer: &mut ratatui::buffer::Buffer) {
        let sidebar_width = if self.sidebar_shown {
            (area.width / 4).clamp(18, 32).min(area.width / 2)
        } else {
            0
        };
        if sidebar_width > 0 {
            let open = self
                .index
                .as_ref()
                .map(|index| (index.account, index.mailbox.as_str()));
            self.sidebar.draw(
                Rect {
                    width: sidebar_width,
                    ..area
                },
                buffer,
                &sidebar::Draw {
                    accounts: &self.accounts,
                    open,
                    focused: self.focus == Focus::Sidebar,
                    theme: &self.theme,
                    icons: &self.icons,
                },
            );
        }
        let index_area = Rect {
            x: area.x + sidebar_width,
            width: area.width - sidebar_width,
            ..area
        };
        self.drawn = (
            Rect {
                width: sidebar_width,
                ..area
            },
            index_area,
        );
        if let Some(index) = &mut self.index {
            index.draw(
                index_area,
                buffer,
                &index::Draw {
                    theme: &self.theme,
                    icons: &self.icons,
                    focused: self.focus == Focus::Index,
                },
            );
        } else {
            let waiting = Line::styled(
                format!(" {}Connecting…", self.icons.loading),
                self.theme.dim,
            );
            buffer.set_line(index_area.x, index_area.y, &waiting, index_area.width);
        }
    }

    /// What the status line says of the Accounts' Tasks, once one's worth saying: a spinner, the Task being done (or
    /// waiting), how long it's taken once that's a while, and how many more are asked.
    fn tasks_said(&self) -> Option<Vec<Span<'static>>> {
        let now = Instant::now();
        let (account, said) = self
            .accounts
            .iter()
            .enumerate()
            .find_map(|(account, state)| Some((account, state.tasks.said(now)?)))?;
        let turned = now.duration_since(self.started).as_millis() / SPIN.as_millis();
        let frame = SPINNER[usize::try_from(turned).unwrap_or(0) % SPINNER.len()];
        let shown = self.index.as_ref().map_or(0, |index| index.account);
        let what = if account == shown {
            said.what.to_owned()
        } else {
            format!("{}: {}", self.accounts[account].name, said.what)
        };
        let mut spans = vec![Span::styled(
            format!("{frame} {}", truncate(&what, 48)),
            self.theme.working,
        )];
        if let Some(since) = said.since
            && now.saturating_duration_since(since) >= TAKING_A_WHILE
        {
            let seconds = now.saturating_duration_since(since).as_secs();
            spans.push(Span::styled(format!(" {seconds}s"), self.theme.dim));
        }
        let more = self
            .accounts
            .iter()
            .map(|state| state.tasks.len())
            .sum::<usize>()
            .saturating_sub(1);
        if more > 0 {
            spans.extend([
                Span::raw(" "),
                Span::styled(format!(" {more} more "), self.theme.queued),
            ]);
        }
        Some(spans)
    }

    /// How often the UI is drawn again while the Accounts have Tasks: the spinner's turn. None while they've none.
    fn ticking(&self) -> Option<Duration> {
        self.accounts
            .iter()
            .any(|state| !state.tasks.is_empty())
            .then_some(SPIN)
    }

    /// What the status line says on its left when there's no notice.
    fn status_left(&self) -> String {
        if self.compose.is_some() {
            return " Draft · y send · e edit · a attach · D detach · P postpone · q discard"
                .into();
        }
        if let Some(pager) = &self.pager {
            return format!(" {}", pager.message().subject);
        }
        self.index_status()
    }

    /// What the status line says of the Index: its Account and Mailbox, how many Messages, and what's selected or
    /// found.
    fn index_status(&self) -> String {
        let Some(index) = &self.index else {
            return " muttrs".into();
        };
        let account = self
            .accounts
            .get(index.account)
            .map_or("", |state| state.name.as_str());
        let counts = self
            .accounts
            .get(index.account)
            .and_then(|state| state.counts.get(&index.mailbox));
        if let Some((query, count)) = &index.found {
            let listed = if index.len() < *count {
                format!(" (newest {} listed)", index.len())
            } else {
                String::new()
            };
            return format!(
                " {account} › {} › “{query}” · {count} found{listed} · Esc back",
                index.label
            );
        }
        let unread = counts.map_or(index.unread(), |counts| counts.unread as usize);
        let messages = if index.loaded {
            let listed = if u32::try_from(index.len()).unwrap_or(u32::MAX) < index.exists {
                format!(" (newest {} listed)", index.len())
            } else {
                String::new()
            };
            let messages = match index.exists {
                1 => "1 Message".to_owned(),
                count => format!("{count} Messages"),
            };
            let selected = match index.selected() {
                0 => String::new(),
                count => format!(" · {count} selected"),
            };
            format!(" · {messages}, {unread} Unread{listed}{selected}")
        } else {
            String::new()
        };
        format!(" {account} › {}{messages}", index.label)
    }

    fn draw_status(&self, area: Rect, frame: &mut Frame<'_>) {
        let buffer = frame.buffer_mut();
        buffer.set_style(area, self.theme.status);
        if let Some(prompt) = &self.prompt {
            let lead = match prompt.kind {
                PromptKind::Search => "/",
                PromptKind::Command => ":",
                PromptKind::Attach => "Attach: ",
                PromptKind::Server => "Search the server: ",
                PromptKind::To => "To: ",
                PromptKind::Subject => "Subject: ",
            };
            let line = Line::from(vec![Span::raw(lead), Span::raw(prompt.text.clone())]);
            buffer.set_line(area.x, area.y, &line, area.width);
            buffer.set_style(area, Style::new());
            let cursor = u16::try_from(lead.width() + prompt.text.width()).unwrap_or(area.width);
            frame.set_cursor_position((area.x + cursor.min(area.width.saturating_sub(1)), area.y));
            return;
        }
        let mut right = Vec::new();
        if self.compose.is_some() {
            // The Draft hides the Pager, and what the Pager would say.
        } else if let Some(pager) = &self.pager {
            if let Some((query, at, of)) = pager.search_status() {
                right.push(if of == 0 {
                    format!("/{query}: no match")
                } else {
                    format!("/{query} {at}/{of}")
                });
            }
            right.push(match pager.showing() {
                pager::Body::Html => "HTML".to_owned(),
                pager::Body::Plain => "Plain".to_owned(),
            });
            right.push(pager.position());
        } else if let Some((query, at, of)) = self.index.as_ref().and_then(Index::search_status) {
            right.push(match (at, of) {
                (_, 0) => format!("/{query}: no match"),
                (Some(at), of) => format!("/{query} {at}/{of}"),
                (None, of) => format!("/{query} ({of})"),
            });
        }
        right.push("? keys".to_owned());
        let mut spans = vec![Span::raw(" ")];
        if let Some(tasks) = self.tasks_said() {
            spans.extend(tasks);
            spans.push(Span::raw(" · "));
        }
        if let Some(label) = self.assessment_label().filter(|_| self.compose.is_none()) {
            spans.extend([label, Span::raw(" · ")]);
        }
        spans.push(Span::raw(format!("{} ", right.join(" · "))));
        let right = Line::from(spans);
        let left = match (&self.notice, &self.pager) {
            (Some(notice), _) => Span::styled(
                format!(" {}", notice.text),
                if notice.error {
                    self.theme.error.patch(self.theme.status)
                } else {
                    self.theme.status
                },
            ),
            (None, _) => Span::raw(self.status_left()),
        };
        let room = usize::from(area.width).saturating_sub(right.width());
        let left = Span::styled(truncate(&left.content, room), left.style);
        buffer.set_line(area.x, area.y, &Line::from(left), area.width);
        let right_width = u16::try_from(right.width())
            .unwrap_or(area.width)
            .min(area.width);
        buffer.set_line(
            area.x + area.width - right_width,
            area.y,
            &right,
            right_width,
        );
    }
}

/// The status line's spinner, while the Accounts have Tasks, and how long each of its frames shows.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPIN: Duration = Duration::from_millis(80);
/// How long a Task's taken before the status line says how long.
const TAKING_A_WHILE: Duration = Duration::from_secs(3);

/// Waits `every`, or for ever when it's None.
async fn tick(every: Option<Duration>) {
    match every {
        Some(every) => tokio::time::sleep(every).await,
        None => std::future::pending().await,
    }
}

/// What sending `draft` is said as, on the status line: its subject, and whom it's to.
fn sending(draft: &muttrs_mail::compose::Draft) -> String {
    let to = draft.to.first().map(|address| {
        address
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| address.email.clone())
    });
    match (draft.subject.trim(), to) {
        ("", Some(to)) => format!("Sending to {to}"),
        ("", None) => "Sending".to_owned(),
        (subject, Some(to)) => format!("Sending {subject} to {to}"),
        (subject, None) => format!("Sending {subject}"),
    }
}

/// `text` cut to `width` cells, with an ellipsis if it's cut.
pub(crate) fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut cut = String::new();
    for character in text.chars() {
        if cut.width() + unicode_width::UnicodeWidthChar::width(character).unwrap_or(0) + 1 > width
        {
            break;
        }
        cut.push(character);
    }
    cut.push('…');
    cut
}

/// The pixel size of the terminal's cells, or 10 × 20 where it doesn't say.
fn cell_size() -> CellSize {
    crossterm::terminal::window_size()
        .ok()
        .filter(|size| size.columns > 0 && size.rows > 0 && size.width > 0 && size.height > 0)
        .map_or(
            CellSize {
                width: 10,
                height: 20,
            },
            |size| CellSize {
                width: u32::from(size.width / size.columns).max(1),
                height: u32::from(size.height / size.rows).max(1),
            },
        )
}

/// Copies `text` to the clipboard, through the terminal (OSC 52), which works over SSH too.
fn copy(text: &str) {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{encoded}\x07");
    let _ = out.flush();
}

/// Saves `bytes` as `name` in `directory`, under a name of its own if that one's taken; where it went.
fn save(directory: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let mut path = directory.join(name);
    let mut copy = 1;
    while path.exists() {
        path = directory.join(format!("{stem} ({copy}){extension}"));
        copy += 1;
    }
    std::fs::write(&path, bytes).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

/// Writes `bytes` to a file of muttrs' own, and opens it with what the system opens such files with.
fn open_attachment(pager: u64, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let directory = xdg::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("attachments")
        .join(format!("{}-{pager}", std::process::id()));
    let path = save(&directory, name, bytes)?;
    open::that_detached(&path).with_context(|| format!("opening {}", path.display()))?;
    Ok(path)
}

/// Shows `message` in the Pager until the user quits: `muttrs view`.
///
/// # Errors
///
/// If the terminal can't be used.
pub(crate) async fn view(setup: Setup, message: Message) -> anyhow::Result<()> {
    let (events, received) = mpsc::unbounded_channel();
    let mut app = App::new(setup, events, Vec::new(), true);
    app.open(Arc::new(message));
    run(app, received).await
}

/// The mail client, on `accounts`, until the user quits.
///
/// # Errors
///
/// If the terminal can't be used, or TLS can't be set up for an Account.
pub(crate) async fn mail(setup: Setup, accounts: Vec<Account>) -> anyhow::Result<()> {
    let (events, received) = mpsc::unbounded_channel();
    let mut states = Vec::new();
    for (index, account) in accounts.into_iter().enumerate() {
        let name = account.name.clone();
        let sender_of_drafts = account.sender();
        let cache = muttrs_mail::cache::Cache::new(
            xdg::cache_dir().map(|cache| cache.join("accounts").join(&name)),
        );
        let sender = events.clone();
        let client = Client::spawn(
            account,
            cache.clone(),
            Arc::new(move |update| {
                let _ = sender.send(Event::Account(index, update));
            }),
        )
        .with_context(|| format!("setting up account {name:?}"))?;
        states.push(AccountState {
            name,
            client,
            connection: Connection::Connecting,
            // As last listed, until the server lists them.
            mailboxes: cache.mailboxes().unwrap_or_default(),
            labels: Vec::new(),
            counts: std::collections::HashMap::new(),
            sender: sender_of_drafts,
            cache,
            tasks: crate::tasks::Tasks::default(),
        });
    }
    let mut app = App::new(setup, events, states, false);
    // The first Account's Inbox, as last listed, while the server's asked.
    if let Some(inbox) = app
        .accounts
        .first()
        .and_then(|state| state.with_role(Role::Inbox))
        .map(|inbox| inbox.name.clone())
    {
        app.open_mailbox(0, &inbox);
    }
    run(app, received).await
}

/// Draws `app` as events come, until it quits.
async fn run(mut app: App, mut received: mpsc::UnboundedReceiver<Event>) -> anyhow::Result<()> {
    let kitty = app.pictures.is_kitty();
    let mouse = app.config.mouse;
    let mut terminal = ratatui::try_init().context("setting up the terminal")?;
    let mut out = io::stdout();
    if mouse {
        let _ = crossterm::execute!(out, EnableMouseCapture);
    }
    let outcome = async {
        if kitty {
            muttrs_gfx::forget_all(&mut out)?;
        }
        let mut input = EventStream::new();
        loop {
            app.pictures.forget_unwanted(&mut out)?;
            out.flush()?;
            app.start_assessment();
            app.bring_back_unsent();
            terminal.draw(|frame| app.draw(frame))?;
            if app.quit {
                return Ok::<_, anyhow::Error>(());
            }
            if std::mem::take(&mut app.edit)
                && let Some(path) = app.compose.as_ref().map(|compose| compose.path.clone())
            {
                // The editor takes the terminal: the input stream lets go of it first, so as not to read its keys.
                drop(input);
                if mouse {
                    let _ = crossterm::execute!(io::stdout(), DisableMouseCapture);
                }
                ratatui::restore();
                let status = compose::edit(&compose::editor(app.config.editor.as_deref()), &path);
                terminal = ratatui::try_init().context("setting up the terminal again")?;
                // Cleared directly: `Terminal::clear` asks the terminal where its cursor is, an answer the
                // input stream that just let go can still take, which leaves the question unanswered.
                crossterm::execute!(
                    io::stdout(),
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                )?;
                if mouse {
                    let _ = crossterm::execute!(io::stdout(), EnableMouseCapture);
                }
                input = EventStream::new();
                app.edited(status);
                continue;
            }
            tokio::select! {
                event = input.next() => match event {
                    Some(Ok(event)) => app.on_terminal(&event),
                    Some(Err(error)) => return Err(error).context("reading the terminal"),
                    None => return Ok(()),
                },
                Some(event) = received.recv() => app.on_event(event, &mut out)?,
                () = tick(app.ticking()) => {}
            }
        }
    }
    .await;
    app.addresses.save();
    if mouse {
        let _ = crossterm::execute!(out, DisableMouseCapture);
    }
    if kitty {
        let _ = muttrs_gfx::forget_all(&mut out);
    }
    ratatui::restore();
    outcome
}

/// The commands `:` completes, in the order offered; those that take more with a space after.
const COMMANDS: [&str; 7] = [
    "select ",
    "unselect ",
    "cd ",
    "refresh",
    "sync!",
    "help",
    "quit",
];

/// The ways to complete the command typed, `text`, each as the whole of it: its name, until there's a space after it;
/// then, for `select` and `unselect`, the filter's last word, by the Messages `listed`.
fn command_completions(text: &str, listed: &[muttrs_mail::Envelope]) -> Vec<String> {
    match text.split_once(' ') {
        None => COMMANDS
            .iter()
            .filter(|command| command.starts_with(text))
            .map(|command| (*command).to_owned())
            .collect(),
        Some((name @ ("select" | "unselect"), filter)) => filter::completions(filter, listed)
            .into_iter()
            .map(|filter| format!("{name} {filter}"))
            .collect(),
        Some(_) => Vec::new(),
    }
}

/// Where the mini-index's rows, its status line and the Pager go in `body`: `lines` rows above the Pager, the status
/// line among them, at most half of `body`, and no more than the Index has `rows` for. None without room for a row
/// and the status line.
fn mini_index(body: Rect, lines: u16, rows: usize) -> Option<(Rect, Rect, Rect)> {
    let shown = (lines.min(body.height / 2))
        .saturating_sub(1)
        .min(u16::try_from(rows).unwrap_or(u16::MAX));
    if shown == 0 {
        return None;
    }
    Some((
        Rect {
            height: shown,
            ..body
        },
        Rect {
            y: body.y + shown,
            height: 1,
            ..body
        },
        Rect {
            y: body.y + shown + 1,
            height: body.height - shown - 1,
            ..body
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_then_filters_complete() {
        assert_eq!(command_completions("s", &[]), ["select ", "sync!"]);
        assert_eq!(command_completions("", &[]).len(), COMMANDS.len());
        assert_eq!(
            command_completions("unselect is:u", &[]),
            Vec::<String>::new(),
            "nothing listed is Unread"
        );
        assert_eq!(command_completions("select -su", &[]), ["select -subject:"]);
        assert_eq!(command_completions("cd In", &[]), Vec::<String>::new());
    }

    #[test]
    fn the_mini_index_above_the_pager() {
        let body = Rect::new(0, 0, 80, 40);
        assert_eq!(
            mini_index(body, 8, 300),
            Some((
                Rect::new(0, 0, 80, 7),
                Rect::new(0, 7, 80, 1),
                Rect::new(0, 8, 80, 32)
            )),
            "7 rows, then the status line"
        );
        assert_eq!(
            mini_index(body, 8, 3).map(|(rows, _, pager)| (rows.height, pager.y)),
            Some((3, 4)),
            "only as many rows as the Index has"
        );
        assert_eq!(
            mini_index(body, 30, 300).map(|(rows, _, _)| rows.height),
            Some(19),
            "at most half"
        );
        assert_eq!(mini_index(body, 0, 300), None);
        assert_eq!(mini_index(body, 1, 300), None, "no room for a row");
        assert_eq!(mini_index(body, 8, 0), None);
    }
}
