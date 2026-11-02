//! The terminal UI: what's on screen, the keys, and the loop that draws it as events come.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use base64::Engine as _;
use crossterm::event::{
    Event as TerminalEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures_util::StreamExt as _;
use muttrs_gfx::{CellSize, Prepared, Protocol};
use muttrs_mail::client::{self, Client, Connection, Flag, Update};
use muttrs_mail::{Account, Message, Role};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr as _;

use crate::compose::{self, Compose};
use crate::config::Config;
use crate::icons::Icons;
use crate::index::{self, Index};
use crate::overlay::{self, Outcome, Overlay, Verb};
use crate::pager::{self, Pager, Request};
use crate::pictures::{self, Pictures};
use crate::remote::Fetcher;
use crate::sidebar::{self, AccountState, Row, Sidebar};
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
}

/// What the UI is set up with.
pub(crate) struct Setup {
    pub(crate) config: Config,
    pub(crate) theme: Theme,
    pub(crate) icons: Icons,
    pub(crate) protocol: Option<Protocol>,
}

/// A line typed at the foot of the screen: a search, or a command.
struct Prompt {
    kind: PromptKind,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Search,
    Command,
    /// The path of a file to attach to the Draft.
    Attach,
    /// What to search the server for.
    Server,
}

/// What's being written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writing {
    New,
    Reply,
    ReplyAll,
    Forward,
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
    overlay: Option<Overlay>,
    prompt: Option<Prompt>,
    notice: Option<Notice>,
    next_pager: u64,
    /// Whether the Pager is all there is, as `muttrs view` opens it.
    standalone: bool,
    /// The Draft being written.
    compose: Option<Compose>,
    /// How many Drafts this run has started, to name their files.
    drafts: u64,
    /// Whether the Draft is to be edited in `$EDITOR`, which the loop does between draws.
    edit: bool,
    /// What to write once the Message being fetched comes, as when replying from the Index.
    write_when_fetched: Option<Writing>,
    /// The first key of a pair typed, as `z` of `za`.
    pending: Option<char>,
    quit: bool,
}

impl App {
    fn new(
        setup: Setup,
        events: mpsc::UnboundedSender<Event>,
        accounts: Vec<AccountState>,
        standalone: bool,
    ) -> Self {
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
            overlay: None,
            prompt: None,
            notice: None,
            next_pager: 1,
            standalone,
            compose: None,
            drafts: 0,
            edit: false,
            write_when_fetched: None,
            pending: None,
            quit: false,
        }
    }

    /// Opens `message` in the Pager.
    fn open(&mut self, message: Arc<Message>) {
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
        self.fulfil(request);
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
                        let text = format!("{}: signing in failed: {why}", state.name);
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
            Update::Counts { mailbox, counts } => {
                state.counts.insert(mailbox, counts);
            }
            Update::Envelopes {
                mailbox,
                envelopes,
                exists,
            } => {
                // While a search's results show, the Mailbox's Index is kept up to date aside.
                let index = match &mut self.searched {
                    Some(searched) => Some(searched),
                    None => self.index.as_mut(),
                };
                if let Some(index) =
                    index.filter(|index| index.account == account && index.mailbox == mailbox)
                {
                    index.set(envelopes, exists);
                }
            }
            Update::Found {
                mailbox,
                query,
                envelopes,
                count,
            } => {
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
            Update::Sent(Ok(())) => {
                if let Some(compose) = self
                    .compose
                    .take_if(|compose| compose.account == account && compose.sending)
                {
                    compose.finish();
                }
                self.say("Sent", false);
            }
            Update::Sent(Err(why)) => {
                if let Some(compose) = &mut self.compose {
                    compose.sending = false;
                }
                self.say(&format!("Not sent: {why}"), true);
            }
        }
    }

    /// Starts writing: a new Message, or one about the Message the Pager shows or the Index's cursor is on.
    fn write(&mut self, writing: Writing) {
        if self.standalone {
            self.say("Writing needs an Account: run muttrs without `view`", true);
            return;
        }
        if writing == Writing::New {
            let account = self.index.as_ref().map_or(0, |index| index.account);
            let Some(state) = self.accounts.get(account) else {
                return;
            };
            let draft = muttrs_mail::compose::Draft::new(state.from.clone());
            self.start(account, draft, None);
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
        let me = state.from.clone();
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
                    me,
                    writing == Writing::ReplyAll,
                    &text,
                ),
                Some((mailbox, uid)),
            ),
            Writing::Forward | Writing::New => (
                muttrs_mail::compose::Draft::forward(message, me, &text),
                None,
            ),
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
        if let Err(error) = compose.reread() {
            self.say(&format!("{error:#}; e edits it again"), true);
            return;
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
        if compose.sending {
            return;
        }
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

    fn send(&mut self) {
        let Some(compose) = &mut self.compose else {
            return;
        };
        let Some(state) = self.accounts.get(compose.account) else {
            return;
        };
        let id = muttrs_mail::compose::message_id(&state.from.email);
        match compose.draft.build(&id) {
            Ok(built) => {
                compose.sending = true;
                state.client.send(client::Request::Send {
                    built: Box::new(built),
                    answers: compose.answers.clone(),
                });
                self.say("Sending…", false);
            }
            Err(error) => self.say(&format!("Not sent: {error}"), true),
        }
    }

    /// Saves the Draft to the Drafts Mailbox, to finish later.
    fn postpone(&mut self) {
        let Some(compose) = &self.compose else { return };
        let Some(state) = self.accounts.get(compose.account) else {
            return;
        };
        let id = muttrs_mail::compose::message_id(&state.from.email);
        match compose.draft.build(&id) {
            Ok(built) => {
                state.client.send(client::Request::Append {
                    role: Role::Drafts,
                    message: built.kept,
                    draft: true,
                });
                if let Some(compose) = self.compose.take() {
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
        let index = Index::new(
            account,
            name.to_owned(),
            mailbox.label().to_owned(),
            mailbox.role,
            self.config.threads,
        );
        state.client.send(client::Request::Open(name.to_owned()));
        self.sidebar
            .select(&self.accounts, &Row::Mailbox(account, name.to_owned()));
        self.searched = None;
        self.index = Some(index);
        self.focus = Focus::Index;
    }

    /// Fetches the Message at the Index's cursor, to show in the Pager, marking it Seen.
    fn open_current(&mut self) {
        let Some(index) = &mut self.index else { return };
        let (account, mailbox) = (index.account, index.mailbox.clone());
        let Some(envelope) = index.current_mut() else {
            return;
        };
        let uid = envelope.uid;
        let unread = !envelope.flags.seen;
        envelope.flags.seen = true;
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        state.client.send(client::Request::Body {
            mailbox: mailbox.clone(),
            uid,
        });
        if unread {
            state.client.send(client::Request::Flag {
                mailbox: mailbox.clone(),
                uids: vec![uid],
                flag: Flag::Seen,
                on: true,
            });
            if let Some(counts) = self.accounts[account].counts.get_mut(&mailbox) {
                counts.unread = counts.unread.saturating_sub(1);
            }
        }
        self.opened = Some(Opened {
            account,
            mailbox,
            uid,
        });
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
            match flag {
                Flag::Seen => envelope.flags.seen = on,
                Flag::Flagged => envelope.flags.flagged = on,
                Flag::Answered => envelope.flags.answered = on,
            }
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
            state.client.send(client::Request::Flag {
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
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        // The worker knows where to: the Mailbox with the Role, by the server or the configuration. Without one it
        // says so, and lists the Mailbox again, which brings the Messages back.
        state.client.send(if archive {
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
        if let Some(state) = self.accounts.get(account) {
            state.client.send(client::Request::Move {
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

    fn on_key(&mut self, key: KeyEvent) {
        self.notice = None;
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.prompt.is_none()
        {
            self.quit = true;
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
                self.quit = true;
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
            && let Some(state) = self.accounts.get(index.account)
        {
            state
                .client
                .send(client::Request::More(index.mailbox.clone()));
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
                    if let Some(state) = self.accounts.get(account) {
                        state.client.send(client::Request::Refresh);
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
            KeyCode::Char('d') => self.remove(false),
            KeyCode::Char('a') => self.remove(true),
            KeyCode::Char('s') => self.choose_mailbox(),
            KeyCode::Char('m') => self.write(Writing::New),
            KeyCode::Char('r') => self.write(Writing::Reply),
            KeyCode::Char('R') => self.write(Writing::ReplyAll),
            KeyCode::Char('f') => self.write(Writing::Forward),
            _ => {}
        }
    }

    fn prompt(&mut self, kind: PromptKind) {
        self.prompt = Some(Prompt {
            kind,
            text: String::new(),
        });
    }

    /// Asks every Account what's new, connecting again any that's offline.
    fn refresh(&mut self) {
        for account in &self.accounts {
            account.client.send(client::Request::Refresh);
        }
        if !self.accounts.is_empty() {
            self.say("Asking for what's new…", false);
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
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Search,
                    text: String::new(),
                });
            }
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt {
                    kind: PromptKind::Command,
                    text: String::new(),
                });
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
            KeyCode::Char('d') => self.remove(false),
            KeyCode::Char('a') => self.remove(true),
            KeyCode::Char('s') => self.choose_mailbox(),
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
        self.pictures.keep(None, |_| false);
        // Standing alone, as `muttrs view` opens it, the Pager is all there is.
        if self.standalone {
            self.quit = true;
        }
    }

    fn prompt_key(&mut self, key: KeyEvent) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
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
            KeyCode::Char(character) => prompt.text.push(character),
            KeyCode::Enter => {
                let Prompt { kind, text } = self.prompt.take().expect("a prompt");
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
                }
            }
            _ => {}
        }
    }

    fn command(&mut self, command: &str) {
        let (name, argument) = command
            .split_once(' ')
            .map_or((command, ""), |(name, argument)| (name, argument.trim()));
        match name {
            "" => {}
            "q" | "quit" | "q!" | "qa" => self.quit = true,
            "help" | "h" => self.help(),
            "refresh" | "sync" => self.refresh(),
            "cd" | "open" if !argument.is_empty() => self.change_mailbox(argument),
            _ => self.say(&format!("Not a command: {command}"), true),
        }
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
        if let Some(state) = self.accounts.get(index.account) {
            state.client.send(client::Request::Search {
                mailbox: index.mailbox.clone(),
                query: query.to_owned(),
            });
            self.say(&format!("Searching {} for {query}…", index.label), false);
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
                        ("d a s", "delete (to Trash), archive, move to a Mailbox"),
                        ("Tab h l", "the sidebar, the Index"),
                        ("\\", "show or hide the sidebar"),
                        ("$ :refresh", "ask for what's new"),
                        (":cd NAME", "open a Mailbox by name"),
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
                    ],
                ),
                (
                    "Anywhere",
                    &[("?", "these keys"), (":q q", "quit, or close the Pager")],
                ),
            ],
        ));
    }

    fn chosen(&mut self, kind: overlay::Kind, index: usize, verb: Verb) {
        // Moving needs no Pager: what's moved is in the Index.
        if kind == overlay::Kind::Mailboxes {
            self.overlay = None;
            self.move_to(index);
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
            overlay::Kind::Mailboxes | overlay::Kind::Help => {}
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
        } else if let Some(pager) = &mut self.pager {
            let mut cx = pager::Cx {
                theme: &self.theme,
                icons: &self.icons,
                pictures: &mut self.pictures,
                pager_width: self.config.pager_width,
                max_picture_rows: self.config.max_picture_rows,
                pixel_ratio: self.config.pixel_ratio,
            };
            pager.draw(body, buffer, &mut cx);
        } else if !self.standalone {
            self.draw_main(body, buffer);
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.draw(body, buffer, &self.theme);
        }
        self.draw_status(status, frame);
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

    /// What the status line says on its left when there's no notice.
    fn status_left(&self) -> String {
        if let Some(compose) = &self.compose {
            return if compose.sending {
                " Sending…".into()
            } else {
                " Draft · y send · e edit · a attach · D detach · P postpone · q discard".into()
            };
        }
        if let Some(pager) = &self.pager {
            return format!(" {}", pager.message().subject);
        }
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
        let right = format!(" {} ", right.join(" · "));
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
            &Line::raw(right),
            right_width,
        );
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
        let from = muttrs_mail::Address {
            name: account.display_name.clone(),
            email: account.email.clone(),
        };
        let cache = xdg::cache_dir().map(|cache| cache.join("accounts").join(&name).join("bodies"));
        let sender = events.clone();
        let client = Client::spawn(
            account,
            muttrs_mail::cache::Cache::new(cache),
            Arc::new(move |update| {
                let _ = sender.send(Event::Account(index, update));
            }),
        )
        .with_context(|| format!("setting up account {name:?}"))?;
        states.push(AccountState {
            name,
            client,
            connection: Connection::Connecting,
            mailboxes: Vec::new(),
            counts: std::collections::HashMap::new(),
            from,
        });
    }
    let app = App::new(setup, events, states, false);
    run(app, received).await
}

/// Draws `app` as events come, until it quits.
async fn run(mut app: App, mut received: mpsc::UnboundedReceiver<Event>) -> anyhow::Result<()> {
    let kitty = app.pictures.is_kitty();
    let mut terminal = ratatui::try_init().context("setting up the terminal")?;
    let mut out = io::stdout();
    let outcome = async {
        if kitty {
            muttrs_gfx::forget_all(&mut out)?;
        }
        let mut input = EventStream::new();
        loop {
            app.pictures.forget_unwanted(&mut out)?;
            out.flush()?;
            terminal.draw(|frame| app.draw(frame))?;
            if app.quit {
                return Ok::<_, anyhow::Error>(());
            }
            if std::mem::take(&mut app.edit)
                && let Some(path) = app.compose.as_ref().map(|compose| compose.path.clone())
            {
                // The editor takes the terminal: the input stream lets go of it first, so as not to read its keys.
                drop(input);
                ratatui::restore();
                let status = compose::edit(&compose::editor(app.config.editor.as_deref()), &path);
                terminal = ratatui::try_init().context("setting up the terminal again")?;
                // Cleared directly: `Terminal::clear` asks the terminal where its cursor is, an answer the
                // input stream that just let go can still take, which leaves the question unanswered.
                crossterm::execute!(
                    io::stdout(),
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                )?;
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
            }
        }
    }
    .await;
    if kitty {
        let _ = muttrs_gfx::forget_all(&mut out);
    }
    ratatui::restore();
    outcome
}
