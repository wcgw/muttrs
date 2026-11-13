//! muttrs in wcgw-kit (its D1–D3): the kit owns the loop, the keys, the `:` commands, the windows, the chrome, the
//! Pictures and the mouse; [`Muttrs`] is the application it runs, with muttrs' Accounts and what they show as the
//! kit's buffers: the sidebar, an Index per open Mailbox, a Pager per Message read, and the Draft.

mod commands;
mod mail;
mod pickers;
#[cfg(test)]
mod tests;
mod voight;
mod writing;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use muttrs_mail::client::{self, Connection, Task, Update};
use muttrs_mail::{Message, Role};
use wcgw_kit::BufferId;
use wcgw_kit::doc::{Cursor, Motion};
use wcgw_kit::shell::{CursorLine, Placement, Size, WindowId};

use crate::addresses::AddressBook;
use crate::assessment::{self, Assessments};
use crate::compose::Compose;
use crate::config::Config;
use crate::hl;
use crate::index::{self, Index};
use crate::pager::{self, Pager, Pictures};
use crate::sidebar::{self, AccountState};
use crate::sign_in::Progress;

/// The kit muttrs runs in.
pub(crate) type Kit = wcgw_kit::Kit<Muttrs>;
/// What muttrs' code returns for the runtime to do.
pub(crate) type Effects = wcgw_kit::Effects<Muttrs>;
/// One thing for the runtime to do.
pub(crate) type Effect = wcgw_kit::Effect<Msg, Io>;
/// A buffer's text, whose spans refer to [`Target`]s.
pub(crate) type Document = wcgw_kit::doc::Document<Target>;
/// One line of a block.
pub(crate) type Line = wcgw_kit::doc::Line<Target>;
/// A run of text in one highlight group.
pub(crate) type Span = wcgw_kit::doc::Span<Target>;

pub(crate) use writing::Writing;

/// A Message of an Account's Mailbox, by its UID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Opened {
    pub(crate) account: usize,
    pub(crate) mailbox: String,
    pub(crate) uid: u32,
}

/// The Messages an action is for: of an Account's Mailbox, by their UIDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Targets {
    pub(crate) account: usize,
    pub(crate) mailbox: String,
    pub(crate) uids: Vec<u32>,
}

/// What muttrs' text refers to: what `<CR>` (`:open`) opens, and what a picker's choices do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// An Account, in the sidebar: opening it asks it what's new.
    Account(usize),
    /// A Mailbox, by its name on the server.
    Mailbox { account: usize, name: String },
    /// A Message of the Index.
    Message(Opened),
    /// A Link in a body, opened in the browser.
    Link(String),
    /// The Draft being written: opening it edits it again.
    Draft,
    /// Moving `targets` to the Mailbox `to`.
    Move { targets: Targets, to: String },
    /// Adding the Account's Label `label` to `targets`, or taking it off.
    Label { targets: Targets, label: usize },
    /// Where a search of every Mailbox found Messages: listing them.
    Found {
        account: usize,
        query: String,
        mailbox: String,
    },
    /// An Attachment of the Pager in `pager`, by its place: opening it opens it as the system does.
    Attachment { pager: BufferId, part: usize },
}

/// What muttrs' own work tells it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Msg {
    /// What an Account says, by its place in the sidebar.
    Account(usize, Update),
    /// The Pager, by its ID, has shown its Message for `pager_read_delay`.
    Read(u64),
    /// Time to turn the spinner, while Tasks wait.
    Tick,
    /// Remote Content was fetched for the image at `source`, this many pixels, its bytes kept with the Pictures;
    /// or it couldn't be.
    Fetched {
        source: String,
        pixels: Option<(u32, u32)>,
    },
    /// What voight says.
    #[cfg_attr(not(feature = "voight"), allow(dead_code))]
    Voight(assessment::Event),
    /// How signing in to an Account again goes, by the sign-in's ID.
    SignIn(u64, Progress),
    /// What unsubscribing by a one-click URL came to.
    Unsubscribed(Result<String, String>),
    /// What the query command found for what was typed.
    Queried(String, Vec<muttrs_mail::Address>),
    /// An Account's address book's contacts, by the Account's name, or why they couldn't be had.
    Contacts(String, Result<Vec<muttrs_mail::Address>, String>),
    /// What came of saving or opening an Attachment, to say.
    Said(Result<String, String>),
}

/// What muttrs asks its runtime to do.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Io {
    /// Fetch the Remote Content at `url`, for the image at `source`, as [`Msg::Fetched`].
    Remote { source: String, url: String },
    /// Unsubscribe by the one-click `url` from `list`, as [`Msg::Unsubscribed`].
    Unsubscribe { url: String, list: String },
    /// Ask the query command for addresses `typed` starts, as [`Msg::Queried`].
    Query { command: String, typed: String },
    /// Sign in to the Account called `account` again, in the browser, as [`Msg::SignIn`] tells.
    SignIn { id: u64, account: String },
    /// Hand the sign-in `id` the address the browser ended at.
    Paste { id: u64, address: String },
    /// Give up the sign-in `id`.
    StopSignIn(u64),
    /// Save `bytes` as `name` in `directory`, as [`Msg::Said`] tells.
    Save {
        directory: std::path::PathBuf,
        name: String,
        bytes: Arc<Vec<u8>>,
    },
    /// Write `bytes` to a file called `name` of muttrs' own, and open it as the system opens such files.
    OpenFile { name: String, bytes: Arc<Vec<u8>> },
    /// Ask voight for the Assessment of `text`.
    Assess { key: assessment::Key, text: String },
}

/// What `u` undoes: Messages moved, archived or deleted to Trash, moved back from where they went.
#[derive(Debug)]
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

/// A Message being fetched to show, or to write about.
#[derive(Debug)]
struct Opening {
    opened: Opened,
    /// The Index it was opened from.
    from: Option<BufferId>,
    /// What's written once it's come, if that's what it's for.
    writing: Option<Writing>,
}

/// Whether muttrs quits once the Accounts' Tasks are done, as quitting with some left asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leaving {
    No,
    /// Asked by the key just pressed.
    Asked,
    /// Waiting: a key pressed now stays.
    Waiting,
}

/// A sign-in under way, and the buffer saying what to do.
#[derive(Debug)]
struct SigningIn {
    id: u64,
    account: usize,
    buffer: BufferId,
}

/// The sidebar: its buffer, its window while it shows, and how wide it is.
#[derive(Debug)]
struct Sidebar {
    buffer: BufferId,
    window: Option<WindowId>,
    width: u16,
}

/// The status line's spinner, while the Accounts have Tasks, and how long each of its frames shows.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPIN: Duration = Duration::from_millis(80);
/// How long a Task's taken before the status line says how long.
const TAKING_A_WHILE: Duration = Duration::from_secs(3);
/// How many Pagers keep their Message, the least recently shown going first: the others show it again once fetched.
const PAGERS_KEPT: usize = 16;

/// What muttrs knows that the editor around it doesn't.
// The flags are each what the UI is doing, not a state machine folded into bools.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Muttrs {
    config: Config,
    pub(crate) accounts: Vec<AccountState>,
    /// Whether a Pager is all there is, as `muttrs view` opens it.
    standalone: bool,
    /// The buffer saying muttrs is connecting: the first, and what fresh windows start on.
    home: BufferId,
    sidebar: Option<Sidebar>,
    /// The window Indexes and Pagers show in.
    main: WindowId,
    /// The window above the Pager keeping some of the Index in sight, as mutt's `pager_index_lines`.
    mini: Option<WindowId>,
    /// The Indexes, by their buffers: the open Mailboxes', and what searches found.
    indexes: HashMap<BufferId, Index>,
    /// Each Mailbox's Index's buffer, by its Account and name on the server.
    mailboxes: HashMap<(usize, String), BufferId>,
    /// What a search of each Mailbox found, by its Account and name on the server.
    searched: HashMap<(usize, String), BufferId>,
    /// The Pagers, by their buffers.
    pagers: HashMap<BufferId, Pager>,
    /// Each Message's Pager's buffer.
    pager_buffers: HashMap<Opened, BufferId>,
    /// The Pagers' buffers, the least recently shown first.
    pagers_shown: Vec<BufferId>,
    next_pager: u64,
    /// The Message being fetched to show or write about.
    opening: Option<Opening>,
    /// The Pager, by its ID, whose Message is marked read once it's shown for `pager_read_delay`.
    reading: Option<u64>,
    /// The Pictures' bytes, which the runtime hands the kit.
    pictures: Pictures,
    /// The Draft being written, and its buffer.
    compose: Option<Compose>,
    draft: Option<BufferId>,
    /// The Drafts being sent, by their Tasks: put away meanwhile, and shown again if they can't be sent.
    sending: Vec<(Task, Compose)>,
    /// The Drafts that couldn't be sent, shown again once no other is.
    unsent: Vec<Compose>,
    /// The Messages that unsubscribe from mailing lists, being sent, by their Account and Task: the list's name.
    unsubscribing: Vec<(usize, Task, String)>,
    /// The Messages being sent on as they are, by their Account and Task: to whom, as written.
    bouncing: Vec<(usize, Task, String)>,
    /// Messages being fetched, to do something with once they've all come.
    piping: Option<mail::Piping>,
    /// What `u` undoes, the last done last.
    undo: Vec<Undo>,
    /// What `u` is undoing, by its Account and Task: kept for `u` again unless it's told moved back.
    undoing: Vec<(usize, Task, Undo)>,
    /// The open Mailbox, being renamed: its Account, and its new path (`Work/Reports`), to open it by.
    renamed: Option<(usize, String)>,
    /// The addresses seen, to complete with.
    addresses: AddressBook,
    /// What the query command found, and for what that was typed.
    queried: Option<(String, Vec<muttrs_mail::Address>)>,
    /// What completing addresses would have the query command asked for, until it's asked.
    to_query: std::cell::RefCell<Option<String>>,
    /// What the query command is asked for, while it is.
    querying: Option<String>,
    /// voight's Assessments, if it's on.
    assessments: Option<Assessments>,
    /// Whether the Pager shows its prose as the Heatmap of its Assessment.
    heatmap: bool,
    /// Signing in to an Account again.
    signing_in: Option<SigningIn>,
    next_sign_in: u64,
    /// When muttrs started: the spinner turns by it.
    started: Instant,
    /// Whether a tick is asked for.
    ticking: bool,
    leaving: Leaving,
    /// The buffers to lay out again before the screen's drawn.
    dirty: HashSet<BufferId>,
    /// The width each Index, Pager and Draft was last laid out for.
    widths: HashMap<BufferId, u16>,
    /// What of the kit's options the buffers were laid out with: icons, and how Pictures are drawn.
    look: Option<(String, wcgw_kit::shell::Graphics)>,
    /// The Index shown last, to go back to.
    last_index: Option<BufferId>,
}

impl Muttrs {
    /// muttrs on `accounts`, set up as `config` says, voight's Assessments kept if it's on: with the kit it runs in,
    /// its groups, icons, commands, keys and statusline, and the sidebar at the left.
    pub(crate) fn mail(
        config: Config,
        accounts: Vec<AccountState>,
        addresses: AddressBook,
        voight: bool,
        pictures: Pictures,
    ) -> (Self, Kit) {
        let (mut muttrs, mut kit) = Self::new(config, accounts, false, voight, pictures);
        muttrs.addresses = addresses;
        let width = sidebar_width(kit.size().0);
        let buffer = kit.add_buffer("mailboxes", Document::new(), false);
        kit.buffer_mut(buffer).set_cursorline(CursorLine::Row);
        let window = kit.open_window(Placement::FarLeft, Size::Cells(width), buffer);
        kit.focus_window(muttrs.main);
        muttrs.sidebar = Some(Sidebar {
            buffer,
            window: Some(window),
            width,
        });
        muttrs.render_sidebar(&mut kit);
        // The first Account's Inbox, as last listed, while the server's asked.
        if let Some(inbox) = muttrs
            .accounts
            .first()
            .and_then(|state| state.with_role(Role::Inbox))
            .map(|inbox| inbox.name.clone())
        {
            muttrs.open_mailbox(&mut kit, 0, &inbox);
        }
        (muttrs, kit)
    }

    /// muttrs showing `message` alone, as `muttrs view` does.
    pub(crate) fn view(
        config: Config,
        message: Message,
        voight: bool,
        pictures: Pictures,
    ) -> (Self, Kit) {
        let (mut muttrs, mut kit) = Self::new(config, Vec::new(), true, voight, pictures);
        let _ = muttrs.show_message(&mut kit, Arc::new(message), None, None);
        (muttrs, kit)
    }

    fn new(
        config: Config,
        accounts: Vec<AccountState>,
        standalone: bool,
        voight: bool,
        pictures: Pictures,
    ) -> (Self, Kit) {
        let mut kit = Kit::new("muttrs", connecting(), false);
        *kit.theme_mut() = hl::theme();
        crate::icons::define(&mut kit);
        commands::add(&mut kit);
        voight::install(&mut kit);
        kit.add_segment(
            wcgw_kit::shell::Segment::new(wcgw_kit::shell::Side::Right, 3, |muttrs: &Self, _| {
                muttrs.tasks_said()
            })
            .with_group(hl::WORKING),
        );
        kit.add_segment(wcgw_kit::shell::Segment::new(
            wcgw_kit::shell::Side::Right,
            1,
            |muttrs: &Self, kit| muttrs.showing(kit),
        ));
        let home = kit.current_buffer();
        let main = kit.current_window();
        let muttrs = Self {
            config,
            accounts,
            standalone,
            home,
            sidebar: None,
            main,
            mini: None,
            indexes: HashMap::new(),
            mailboxes: HashMap::new(),
            searched: HashMap::new(),
            pagers: HashMap::new(),
            pager_buffers: HashMap::new(),
            pagers_shown: Vec::new(),
            next_pager: 1,
            opening: None,
            reading: None,
            pictures,
            compose: None,
            draft: None,
            sending: Vec::new(),
            unsent: Vec::new(),
            unsubscribing: Vec::new(),
            bouncing: Vec::new(),
            piping: None,
            undo: Vec::new(),
            undoing: Vec::new(),
            renamed: None,
            addresses: AddressBook::default(),
            queried: None,
            to_query: std::cell::RefCell::new(None),
            querying: None,
            assessments: voight.then(Assessments::default),
            heatmap: false,
            signing_in: None,
            next_sign_in: 1,
            started: Instant::now(),
            ticking: false,
            leaving: Leaving::No,
            dirty: HashSet::new(),
            widths: HashMap::new(),
            look: None,
            last_index: None,
        };
        (muttrs, kit)
    }

    // --- Windows. ---

    /// The window Indexes and Pagers show in: the one muttrs opened, else any but the sidebar's, else a new one
    /// beside the sidebar.
    fn main_window(&mut self, kit: &mut Kit) -> WindowId {
        if kit.window_buffer(self.main).is_some() && Some(self.main) != self.sidebar_window() {
            return self.main;
        }
        let sidebar = self.sidebar_window();
        let other = kit
            .window_ids()
            .into_iter()
            .find(|&id| Some(id) != sidebar && Some(id) != self.mini);
        self.main = if let Some(id) = other {
            id
        } else {
            let window = kit.open_window(Placement::Right, Size::Shared, self.home);
            if let Some(sidebar) = &mut self.sidebar {
                // The only window was the sidebar's: it's laid out again with room beside it.
                sidebar.window = sidebar.window.filter(|&id| kit.window_buffer(id).is_some());
            }
            window
        };
        self.main
    }

    /// The sidebar's window, while it shows.
    fn sidebar_window(&self) -> Option<WindowId> {
        self.sidebar.as_ref().and_then(|sidebar| sidebar.window)
    }

    /// Shows `buffer` in the main window, which becomes current.
    fn show_in_main(&mut self, kit: &mut Kit, buffer: BufferId) {
        let main = self.main_window(kit);
        kit.focus_window(main);
        kit.show_buffer(buffer);
    }

    /// Puts the cursor of `window` on line `line` of the block `key`, or on the block's row while it's folded.
    fn put_cursor(kit: &mut Kit, window: WindowId, key: &str, line: usize) {
        let current = kit.current_window();
        if !kit.focus_window(window) {
            return;
        }
        kit.set_cursor(window, Cursor::on_block(key));
        if line > 0
            && !kit.on_closed_fold()
            && let Some(mut cursor) = kit.window_cursor(window).cloned()
        {
            cursor.apply(&kit.current_layout(), Motion::Down, line);
            kit.set_cursor(window, cursor);
        }
        kit.focus_window(current);
    }

    /// Puts the cursor of `window`, showing the Index in `buffer`, on Message `uid`.
    fn put_cursor_on_message(&self, kit: &mut Kit, window: WindowId, buffer: BufferId, uid: u32) {
        if let Some((key, line)) = self
            .indexes
            .get(&buffer)
            .and_then(|index| index.place_of(uid))
        {
            Self::put_cursor(kit, window, &key, line);
        }
    }

    /// `:sidebar` (`\`): hides the sidebar from within it, goes to it from anywhere else, and brings it back, as
    /// slackers' does.
    pub(crate) fn toggle_sidebar(&mut self, kit: &mut Kit) {
        let Some(sidebar) = &mut self.sidebar else {
            return;
        };
        match sidebar
            .window
            .filter(|&window| kit.window_buffer(window).is_some())
        {
            Some(window) if kit.current_window() == window => {
                if kit.close_window_id(window) {
                    sidebar.window = None;
                }
            }
            Some(window) => {
                kit.focus_window(window);
            }
            None => {
                let window = kit.open_window(
                    Placement::FarLeft,
                    Size::Cells(sidebar.width),
                    sidebar.buffer,
                );
                sidebar.window = Some(window);
            }
        }
    }

    // --- Laying buffers out. ---

    /// Notes every buffer to be laid out again, for a width or a look of another.
    fn everything_dirty(&mut self) {
        self.dirty.extend(self.indexes.keys().copied());
        self.dirty.extend(self.pagers.keys().copied());
        self.dirty.extend(self.draft);
        if let Some(sidebar) = &self.sidebar {
            self.dirty.insert(sidebar.buffer);
        }
    }

    /// Lays out again what changed, and what's to follow from a change: the spinner's turns, voight's next
    /// Assessment, a Draft that couldn't be sent.
    fn settle(&mut self, kit: &mut Kit) -> Effects {
        // What the kit's options change of what's drawn: icons are in the text, and Pictures take room or not.
        let look = (kit.icon(crate::icons::UNREAD), kit.graphics());
        if self.look.as_ref() != Some(&look) {
            self.look = Some(look);
            self.everything_dirty();
        }
        // Laid out again for the width they're shown at now.
        let shown: Vec<BufferId> = self
            .indexes
            .keys()
            .chain(self.pagers.keys())
            .chain(self.draft.iter())
            .copied()
            .filter(|&buffer| {
                !kit.windows_showing(buffer).is_empty()
                    && self.widths.get(&buffer) != Some(&kit.text_width(buffer))
            })
            .collect();
        self.dirty.extend(shown);
        self.bring_back_unsent(kit);
        for buffer in std::mem::take(&mut self.dirty) {
            if self.indexes.contains_key(&buffer) {
                self.render_index(kit, buffer);
            } else if self.pagers.contains_key(&buffer) {
                self.render_pager(kit, buffer);
            } else if Some(buffer) == self.draft {
                self.render_draft(kit);
            } else if self
                .sidebar
                .as_ref()
                .is_some_and(|sidebar| sidebar.buffer == buffer)
            {
                self.render_sidebar(kit);
            }
        }
        let mut effects = self.start_assessment(kit);
        let waiting = self.accounts.iter().any(|state| !state.tasks.is_empty());
        if waiting && !self.ticking {
            self.ticking = true;
            effects.push(Effect::After {
                after: SPIN,
                msg: Msg::Tick,
            });
        }
        effects
    }

    fn render_sidebar(&self, kit: &mut Kit) {
        let Some(sidebar) = &self.sidebar else {
            return;
        };
        let open = self
            .current_index(kit)
            .map(|(_, index)| (index.account, index.mailbox.clone()));
        let open_style = kit.style(hl::OPEN);
        let blocks = {
            let icon = |name: &str| kit.icon(name);
            sidebar::blocks(&sidebar::Draw {
                accounts: &self.accounts,
                open: open
                    .as_ref()
                    .map(|(account, name)| (*account, name.as_str())),
                width: usize::from(sidebar.width),
                open_style,
                icon: &icon,
            })
        };
        kit.buffer_mut(sidebar.buffer).doc_mut().set_blocks(blocks);
    }

    fn render_index(&mut self, kit: &mut Kit, buffer: BufferId) {
        let Some(index) = self.indexes.get(&buffer) else {
            return;
        };
        let look = index::Look::of(kit);
        let width = kit.text_width(buffer);
        self.widths.insert(buffer, width);
        let blocks = index.blocks(usize::from(width.max(20)), &look);
        let winbar = self.index_winbar(index);
        let buffer = kit.buffer_mut(buffer);
        buffer.doc_mut().set_blocks(blocks);
        buffer.set_winbar(Some(vec![(winbar, wcgw_kit::hl::Group::WIN_BAR)]));
    }

    /// What the winbar of `index` says: its Account and Mailbox, how many Messages, and what's selected or found.
    fn index_winbar(&self, index: &Index) -> String {
        let account = self
            .accounts
            .get(index.account)
            .map_or("", |state| state.name.as_str());
        if let Some((query, count)) = &index.found {
            let listed = if index.len() < *count {
                format!(" (newest {} listed)", index.len())
            } else {
                String::new()
            };
            return format!(
                " {account} › {} › “{query}” · {count} found{listed} · q back",
                index.label
            );
        }
        let counts = self
            .accounts
            .get(index.account)
            .and_then(|state| state.counts.get(&index.mailbox));
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
            let limited = index
                .limited()
                .map_or_else(String::new, |filter| format!(" · limited to {filter}"));
            let sorted = index
                .sorted()
                .map_or_else(String::new, |sorted| format!(" · {sorted}"));
            format!(" · {messages}, {unread} Unread{listed}{sorted}{limited}{selected}")
        } else {
            String::new()
        };
        format!(" {account} › {}{messages}", index.label)
    }

    /// The Pager's look, from the kit.
    fn pager_look(&self, kit: &Kit) -> pager::Look {
        let (width, height) = kit.cell_size();
        pager::Look {
            heat: crate::heatmap::Heat {
                human: kit.style(hl::HEAT_HUMAN),
                even: kit.style(hl::HEAT_EVEN),
                llm: kit.style(hl::HEAT_LLM),
            },
            attachment: kit.icon(crate::icons::ATTACHMENT),
            blocked: kit.icon(crate::icons::BLOCKED),
            cell: wcgw_kit_text::CellSize {
                width: u32::from(width),
                height: u32::from(height),
            },
            pixel_ratio: self.config.pixel_ratio,
            max_picture_rows: self.config.max_picture_rows,
            pictures: kit.graphics() != wcgw_kit::shell::Graphics::Off,
        }
    }

    /// The styles bodies are drawn in: the colorscheme's, authors' colours judged against `MuttrsPage`.
    fn body_styles(kit: &Kit) -> wcgw_kit_text::Styles {
        let mut styles = wcgw_kit_text::kit::styles(
            kit.theme(),
            wcgw_kit_text::kit::page_of(kit.style(hl::PAGE)),
        );
        styles.glyphs.image = kit.icon(crate::icons::PICTURE);
        styles
    }

    /// The width the Pager or the Draft in `buffer` lays bodies out for: its window's, up to `pager_width`.
    fn pager_width(&mut self, kit: &Kit, buffer: BufferId) -> u16 {
        let width = kit.text_width(buffer);
        self.widths.insert(buffer, width);
        width.clamp(20, self.config.pager_width.max(20))
    }

    fn render_pager(&mut self, kit: &mut Kit, buffer: BufferId) {
        let width = self.pager_width(kit, buffer);
        let Some(pager) = self.pagers.get(&buffer) else {
            return;
        };
        let look = self.pager_look(kit);
        let heat = self.heat(pager);
        let blocks = pager.blocks(width, &look, heat.as_deref());
        let subject = if pager.message().subject.is_empty() {
            "(no subject)".to_owned()
        } else {
            pager.message().subject.clone()
        };
        let buffer = kit.buffer_mut(buffer);
        buffer.doc_mut().set_blocks(blocks);
        buffer.set_winbar(Some(vec![(format!(" {subject}"), hl::SUBJECT)]));
    }

    fn render_draft(&mut self, kit: &mut Kit) {
        let Some(buffer) = self.draft else {
            return;
        };
        let width = self.pager_width(kit, buffer);
        let Some(compose) = &self.compose else {
            return;
        };
        let styles = Self::body_styles(kit);
        let attachment = kit.icon(crate::icons::ATTACHMENT);
        let blocks = compose.blocks(width, &styles, &attachment);
        let buffer = kit.buffer_mut(buffer);
        buffer.doc_mut().set_blocks(blocks);
        buffer.set_winbar(Some(vec![(
            " Draft · :send (<Space>s) · <CR> edits · :attach FILE · :detach · :postpone (<Space>p) · q discards"
                .to_owned(),
            wcgw_kit::hl::Group::WIN_BAR,
        )]));
    }

    // --- Mailboxes. ---

    /// Shows the Index of `account`'s Mailbox `name`, in the main window.
    pub(crate) fn open_mailbox(&mut self, kit: &mut Kit, account: usize, name: &str) {
        let Some(state) = self.accounts.get(account) else {
            return;
        };
        let Some(mailbox) = state.mailbox(name) else {
            return;
        };
        if !mailbox.selectable {
            kit.info(format!("{} only holds other Mailboxes", mailbox.label()));
            return;
        }
        let key = (account, name.to_owned());
        let buffer = if let Some(&buffer) = self.mailboxes.get(&key) {
            buffer
        } else {
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
            let shown = if self.accounts.len() > 1 {
                format!("{}/{}", state.name, mailbox.label())
            } else {
                mailbox.label().to_owned()
            };
            let buffer = kit.add_buffer(shown, Document::new(), true);
            kit.buffer_mut(buffer).set_cursorline(CursorLine::Row);
            self.indexes.insert(buffer, index);
            self.mailboxes.insert(key, buffer);
            buffer
        };
        self.accounts[account].ask(client::Request::Open(name.to_owned()));
        // Opened from elsewhere than the sidebar, the sidebar's cursor goes to it too.
        if let Some(window) = self
            .sidebar_window()
            .filter(|&id| id != kit.current_window())
            && let Some((key, line)) = sidebar::place_of(&self.accounts, account, name)
        {
            Self::put_cursor(kit, window, &key, line);
        }
        self.close_mini(kit);
        self.show_in_main(kit, buffer);
        self.dirty.insert(buffer);
        if let Some(sidebar) = &self.sidebar {
            self.dirty.insert(sidebar.buffer);
        }
    }

    /// The Index the current buffer is, or the Pager's came from; from the sidebar, the main window's.
    fn current_index(&self, kit: &Kit) -> Option<(BufferId, &Index)> {
        let mut current = kit.current_buffer();
        if self
            .sidebar
            .as_ref()
            .is_some_and(|sidebar| sidebar.buffer == current)
        {
            current = kit.window_buffer(self.main)?;
        }
        let buffer = if self.indexes.contains_key(&current) {
            current
        } else {
            self.pagers.get(&current)?.from?
        };
        Some((buffer, self.indexes.get(&buffer)?))
    }

    /// The Account shown: the current Index's, else the first.
    fn account_shown(&self, kit: &Kit) -> usize {
        self.current_index(kit)
            .map_or(0, |(_, index)| index.account)
    }

    /// The Index of `account`'s Mailbox `mailbox`, if it's open, with its buffer.
    fn index_of_mut(&mut self, account: usize, mailbox: &str) -> Option<(BufferId, &mut Index)> {
        let buffer = *self.mailboxes.get(&(account, mailbox.to_owned()))?;
        Some((buffer, self.indexes.get_mut(&buffer)?))
    }

    // --- What Accounts say. ---

    fn on_account(&mut self, kit: &mut Kit, account: usize, update: Update) -> Effects {
        let Some(state) = self.accounts.get_mut(account) else {
            return Vec::new();
        };
        if let Some(sidebar) = &self.sidebar {
            self.dirty.insert(sidebar.buffer);
        }
        match update {
            Update::Connection(connection) => {
                match &connection {
                    Connection::SignInFailed(why) => {
                        let name = state.name.clone();
                        // The hint goes before the reason, which the message line may cut short.
                        let text = if self.signs_in_with_oauth2(&name) {
                            format!(
                                "{name}: signing in failed (`:login {name}` signs in again): {why}"
                            )
                        } else {
                            format!("{name}: signing in failed: {why}")
                        };
                        kit.error(text);
                    }
                    Connection::Offline(why) if state.connection == Connection::Online => {
                        kit.error(format!("{}: offline: {why}", state.name));
                    }
                    _ => {}
                }
                if let Some(state) = self.accounts.get_mut(account) {
                    state.connection = connection;
                }
            }
            Update::Mailboxes(mailboxes) => self.listed(kit, account, mailboxes),
            Update::Labels(labels) => state.labels = labels,
            Update::Counts { mailbox, counts } => {
                state.counts.insert(mailbox.clone(), counts);
                if let Some(&buffer) = self.mailboxes.get(&(account, mailbox)) {
                    self.dirty.insert(buffer);
                }
            }
            Update::Envelopes {
                mailbox,
                envelopes,
                exists,
            } => {
                self.addresses.learn(envelopes.iter().flat_map(|envelope| {
                    envelope.from.iter().chain(&envelope.to).chain(&envelope.cc)
                }));
                // As they'll be once what's asked is done: what the server tells may be from before.
                let mut envelopes = envelopes;
                state.tasks.apply(&mailbox, &mut envelopes);
                if let Some((buffer, index)) = self.index_of_mut(account, &mailbox) {
                    index.set(envelopes, exists);
                    self.dirty.insert(buffer);
                }
            }
            Update::Found {
                mailbox,
                query,
                mut envelopes,
                count,
            } => {
                state.tasks.apply(&mailbox, &mut envelopes);
                self.found(kit, account, &mailbox, &query, envelopes, count);
            }
            Update::Body { mailbox, uid, body } => {
                return self.body(kit, account, mailbox, uid, body);
            }
            Update::FoundIn { query, found } => {
                return self.found_in(kit, account, &query, found);
            }
            Update::Done(done) => kit.info(done),
            Update::Failed(failed) => kit.error(failed),
            Update::Started(task) => state.tasks.started(task, Instant::now()),
            Update::Finished(task) => {
                state.tasks.finished(task);
                self.not_undone(kit, account, task);
                if self.leaving != Leaving::No && self.tasks_done() {
                    return vec![Effect::Quit];
                }
            }
            Update::MovedBack(task) => self
                .undoing
                .retain(|(of, undoing, _)| (*of, *undoing) != (account, task)),
            Update::Sent { task, sent } => self.sent(kit, account, task, sent),
        }
        Vec::new()
    }

    /// Takes `account`'s Mailboxes, as the server lists them: the open one renamed is opened by its new name, and the
    /// first Account's Inbox, to start with.
    fn listed(&mut self, kit: &mut Kit, account: usize, mailboxes: Vec<muttrs_mail::Mailbox>) {
        let Some(state) = self.accounts.get_mut(account) else {
            return;
        };
        state.mailboxes = mailboxes;
        if let Some((of, path)) = self.renamed.take_if(|(of, _)| *of == account) {
            match state
                .mailboxes
                .iter()
                .find(|mailbox| mailbox.path.join("/") == path)
            {
                Some(renamed) => {
                    let name = renamed.name.clone();
                    self.open_mailbox(kit, account, &name);
                    return;
                }
                None => self.renamed = Some((of, path)),
            }
        }
        if self.mailboxes.is_empty()
            && let Some(inbox) = state.with_role(Role::Inbox).map(|inbox| inbox.name.clone())
        {
            self.open_mailbox(kit, account, &inbox);
        }
    }

    /// Whether the Account called `name` signs in with OAuth2, and so can sign in again with `:login`.
    fn signs_in_with_oauth2(&self, name: &str) -> bool {
        crate::login::account(&self.config, name)
            .is_ok_and(|account| matches!(account.auth, muttrs_mail::account::Auth::OAuth2(_)))
    }

    // --- Reading. ---

    /// Fetches `opened` to show in the Pager, or to write about. It's marked read at once when `pager_read_delay` is
    /// 0, or when it's fetched to write about; otherwise once it's shown for that long.
    fn open_message(
        &mut self,
        kit: &mut Kit,
        opened: Opened,
        from: Option<BufferId>,
        writing: Option<Writing>,
    ) {
        let Some(state) = self.accounts.get_mut(opened.account) else {
            return;
        };
        state.ask(client::Request::Body {
            mailbox: opened.mailbox.clone(),
            uid: opened.uid,
        });
        // The Message shown before, if it waited to be marked read, stays Unread.
        self.reading = None;
        if self.config.pager_read_delay == 0 || writing.is_some() {
            self.mark_seen(&opened);
        }
        if writing.is_none() && !self.pagers.contains_key(&kit.current_buffer()) {
            kit.info("Fetching the Message…");
        }
        self.opening = Some(Opening {
            opened,
            from,
            writing,
        });
    }

    /// Takes Message `uid` of `account`'s `mailbox`, fetched: shown, written about, or piped, if it's wanted.
    fn body(
        &mut self,
        kit: &mut Kit,
        account: usize,
        mailbox: String,
        uid: u32,
        body: Result<Arc<Vec<u8>>, String>,
    ) -> Effects {
        let mut effects = self.pipe_when_fetched(kit, account, &mailbox, uid, &body);
        let wanted = Opened {
            account,
            mailbox,
            uid,
        };
        if self
            .opening
            .as_ref()
            .is_none_or(|opening| opening.opened != wanted)
        {
            return effects;
        }
        let Some(opening) = self.opening.take() else {
            return effects;
        };
        match body {
            Ok(bytes) => match Message::parse(bytes.as_ref().clone()) {
                Ok(message) => {
                    let message = Arc::new(message);
                    match opening.writing {
                        Some(writing) => {
                            effects.extend(self.write_about(kit, writing, account, &message, uid));
                        }
                        None => effects.extend(self.show_message(
                            kit,
                            message,
                            Some(wanted),
                            opening.from,
                        )),
                    }
                }
                Err(error) => kit.error(error.to_string()),
            },
            Err(why) => kit.error(format!("Couldn't fetch the Message: {why}")),
        }
        effects
    }

    /// Shows `message` in a Pager, in the main window: `opened`, opened from the Index in `from`.
    fn show_message(
        &mut self,
        kit: &mut Kit,
        message: Arc<Message>,
        opened: Option<Opened>,
        from: Option<BufferId>,
    ) -> Effects {
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
        let styles = Self::body_styles(kit);
        let mut pager = Pager::new(
            id,
            message,
            &styles,
            self.config.prefer,
            self.config.load_remote_content,
            &self.pictures,
        );
        let uid = opened.as_ref().map(|opened| opened.uid);
        let buffer = if let Some(&buffer) = opened
            .as_ref()
            .and_then(|opened| self.pager_buffers.get(opened))
        {
            buffer
        } else {
            let subject = truncate_name(&pager.message().subject);
            let buffer = kit.add_buffer(subject, Document::new(), self.standalone);
            if let Some(opened) = &opened {
                self.pager_buffers.insert(opened.clone(), buffer);
            }
            buffer
        };
        pager.opened = opened;
        pager.from = from;
        let fetch = pager.remote_to_fetch();
        self.pagers.insert(buffer, pager);
        self.kept(kit, buffer);
        self.render_pager(kit, buffer);
        if self.standalone {
            kit.show_buffer(buffer);
        } else {
            self.show_in_main(kit, buffer);
            self.show_mini(kit, from, uid);
            let main = self.main;
            kit.focus_window(main);
        }
        let mut effects = self.read_later(id);
        if let pager::Request::Fetch(images) = fetch {
            effects.extend(
                images
                    .into_iter()
                    .map(|(source, url)| Effect::App(Io::Remote { source, url })),
            );
        }
        effects
    }

    /// Notes the Pager in `buffer` as the last shown, letting go of the Message of the least recently shown of more
    /// than [`PAGERS_KEPT`].
    fn kept(&mut self, kit: &mut Kit, buffer: BufferId) {
        self.pagers_shown.retain(|&shown| shown != buffer);
        self.pagers_shown.push(buffer);
        while self.pagers_shown.len() > PAGERS_KEPT {
            let gone = self.pagers_shown.remove(0);
            if let Some(pager) = self.pagers.remove(&gone) {
                self.forget_pictures(&pager);
                let mut doc = Document::new();
                doc.push_back(
                    "gone",
                    vec![Line::Text(vec![
                        Span::new(
                            "This Message was let go of: <CR> fetches it again.",
                            hl::DIM,
                        )
                        .with_target(pager.opened.clone().map_or(Target::Draft, Target::Message)),
                    ])],
                );
                *kit.buffer_mut(gone).doc_mut() = doc;
            }
        }
    }

    /// Lets go of the bytes of `pager`'s Pictures.
    fn forget_pictures(&self, pager: &Pager) {
        let prefix = format!("muttrs:{}/", pager.id());
        self.pictures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|source, _| !source.starts_with(&prefix));
    }

    /// Keeps some of the Index from `from` in sight above the Pager, `pager_index_lines` rows of it, its cursor on
    /// Message `uid`, as mutt's mini-index.
    fn show_mini(&mut self, kit: &mut Kit, from: Option<BufferId>, uid: Option<u32>) {
        let lines = self.config.pager_index_lines;
        let (Some(from), Some(uid)) = (from, uid) else {
            return;
        };
        if lines < 2 {
            return;
        }
        let main = self.main_window(kit);
        let open = self
            .mini
            .filter(|&mini| kit.window_buffer(mini) == Some(from));
        let window = if let Some(mini) = open {
            mini
        } else {
            self.close_mini(kit);
            kit.focus_window(main);
            // A window's own row, between it and the Pager, is the mini-index's status line, as mutt has it.
            let window = kit.open_window(Placement::Above, Size::Cells(lines), from);
            self.mini = Some(window);
            window
        };
        self.put_cursor_on_message(kit, window, from, uid);
        kit.focus_window(main);
    }

    fn close_mini(&mut self, kit: &mut Kit) {
        if let Some(mini) = self.mini.take() {
            kit.close_window_id(mini);
        }
    }

    /// Marks the opened Message read once the Pager `pager` has shown it for `pager_read_delay`, if it's Unread.
    fn read_later(&mut self, pager: u64) -> Effects {
        self.reading = None;
        let delay = self.config.pager_read_delay;
        let opened = self
            .pagers
            .values()
            .find(|shown| shown.id() == pager)
            .and_then(|shown| shown.opened.clone());
        if delay == 0 || !opened.as_ref().is_some_and(|opened| self.is_unread(opened)) {
            return Vec::new();
        }
        self.reading = Some(pager);
        vec![Effect::After {
            after: Duration::from_secs(delay),
            msg: Msg::Read(pager),
        }]
    }

    /// Whether the Index lists `opened`, Unread.
    fn is_unread(&self, opened: &Opened) -> bool {
        self.mailboxes
            .get(&(opened.account, opened.mailbox.clone()))
            .and_then(|buffer| self.indexes.get(buffer))
            .and_then(|index| index.find(opened.uid))
            .is_some_and(|envelope| !envelope.flags.seen)
    }

    /// Marks `opened` read, in the Index, its Mailbox's counts and on the server, if it's Unread.
    fn mark_seen(&mut self, opened: &Opened) {
        let Some((buffer, index)) = self.index_of_mut(opened.account, &opened.mailbox) else {
            return;
        };
        let Some(envelope) = index.find_mut(opened.uid) else {
            return;
        };
        if envelope.flags.seen {
            return;
        }
        envelope.flags.seen = true;
        self.dirty.insert(buffer);
        let Some(state) = self.accounts.get_mut(opened.account) else {
            return;
        };
        state.ask(client::Request::Flag {
            mailbox: opened.mailbox.clone(),
            uids: vec![opened.uid],
            flag: client::Flag::Seen,
            on: true,
        });
        if let Some(counts) = state.counts.get_mut(&opened.mailbox) {
            counts.unread = counts.unread.saturating_sub(1);
        }
    }

    /// `q` in a Pager: back to the Index it was opened from, its cursor on the Message shown.
    fn close_pager(&mut self, kit: &mut Kit, buffer: BufferId) -> Effects {
        if self.standalone {
            return vec![Effect::Quit];
        }
        self.reading = None;
        self.close_mini(kit);
        let Some(pager) = self.pagers.get(&buffer) else {
            return Vec::new();
        };
        let shown = pager.opened.as_ref().map(|opened| opened.uid);
        let Some(from) = pager.from.filter(|from| self.indexes.contains_key(from)) else {
            kit.show_buffer(self.home);
            return Vec::new();
        };
        let main = self.main_window(kit);
        kit.focus_window(main);
        kit.show_buffer(from);
        // On the Message shown last, which `<C-n>` may have gone on to.
        if let Some(uid) = shown {
            self.put_cursor_on_message(kit, main, from, uid);
        }
        self.dirty.insert(from);
        Vec::new()
    }

    /// `<C-n>`, `<C-p>` in the Pager: opens the next Message, older, or the previous one, newer.
    pub(crate) fn step(&mut self, kit: &mut Kit, older: bool) -> Result<Effects, String> {
        let current = kit.current_buffer();
        let Some(pager) = self.pagers.get(&current) else {
            return Err("That's for the Pager".to_owned());
        };
        let (Some(opened), Some(from)) = (pager.opened.clone(), pager.from) else {
            return Err("That's for a Message of a Mailbox".to_owned());
        };
        let Some(index) = self.indexes.get(&from) else {
            return Ok(Vec::new());
        };
        let Some(next) = index.next(opened.uid, older) else {
            return Err(if older {
                "No older Message".to_owned()
            } else {
                "No newer Message".to_owned()
            });
        };
        let opened = Opened {
            uid: next,
            ..opened
        };
        self.open_message(kit, opened, Some(from), None);
        Ok(self.more_if_wanted(from, true))
    }

    /// Asks for older Envelopes for the Index in `buffer`, if its cursor is `near_end` of those it lists.
    fn more_if_wanted(&mut self, buffer: BufferId, near_end: bool) -> Effects {
        let Some(index) = self.indexes.get_mut(&buffer) else {
            return Vec::new();
        };
        if index.found.is_none()
            && index.wants_more(near_end)
            && let Some(state) = self.accounts.get_mut(index.account)
        {
            state.ask(client::Request::More(index.mailbox.clone()));
        }
        Vec::new()
    }

    /// The windows of the Assessment of the prose `pager` shows, while the Heatmap shows and it's made.
    fn heat(&self, pager: &Pager) -> Option<Arc<[assessment::Window]>> {
        if !self.heatmap {
            return None;
        }
        match self.assessments.as_ref()?.get(pager.prose()?)? {
            assessment::Assessed::Done(Some(assessment)) => Some(Arc::clone(&assessment.windows)),
            _ => None,
        }
    }

    /// Asks voight for the Assessment of the prose the Pager shows, once it's ready and assessing nothing else.
    fn start_assessment(&mut self, kit: &Kit) -> Effects {
        let Some(assessments) = &mut self.assessments else {
            return Vec::new();
        };
        let Some(prose) = self
            .pagers
            .get(&kit.current_buffer())
            .and_then(Pager::prose)
        else {
            return Vec::new();
        };
        if !assessments.can_assess() || assessments.get(prose).is_some() {
            return Vec::new();
        }
        assessments.assessing(Arc::clone(prose));
        vec![Effect::App(Io::Assess {
            key: Arc::clone(prose),
            text: prose.to_string(),
        })]
    }

    // --- Quitting. ---

    fn tasks_done(&self) -> bool {
        self.accounts.iter().all(|state| state.tasks.is_empty())
    }

    /// `q` where there's nothing to close: `:qa`, which quits once the Accounts' Tasks are done.
    fn ask_to_quit(&mut self, kit: &mut Kit) -> Effects {
        kit.run_ex(self, "qa").unwrap_or_else(|why| {
            kit.error(why);
            Vec::new()
        })
    }

    /// Whether to quit now, or once the Accounts' Tasks are done, unless a key is pressed first: what's flagged,
    /// moved or sent goes to the server before muttrs quits.
    fn leave(&mut self, kit: &mut Kit) -> wcgw_kit::Quitting {
        if self.tasks_done() {
            return wcgw_kit::Quitting::Now;
        }
        self.leaving = Leaving::Asked;
        let left: usize = self.accounts.iter().map(|state| state.tasks.len()).sum();
        let tasks = if left == 1 {
            "the Task left is".to_owned()
        } else {
            format!("the {left} Tasks left are")
        };
        kit.info(format!(
            "Quitting once {tasks} done: a key stays, :qa! quits now"
        ));
        wcgw_kit::Quitting::Later
    }

    /// The addresses seen, to keep once muttrs has quit.
    pub(crate) fn save(&mut self) {
        self.addresses.save();
    }

    /// `q`: closes what's current (the Pager, back to its Index; the Draft, once confirmed; what a search found;
    /// signing in), else quits once what's asked is done.
    pub(crate) fn exit(&mut self, kit: &mut Kit) -> Effects {
        let current = kit.current_buffer();
        if self.pagers.contains_key(&current) {
            return self.close_pager(kit, current);
        }
        if Some(current) == self.draft {
            return Self::ask_to_discard(kit);
        }
        if let Some(signing_in) = self
            .signing_in
            .as_ref()
            .filter(|signing_in| signing_in.buffer == current)
        {
            let id = signing_in.id;
            self.signing_in = None;
            self.back_from(kit);
            return vec![Effect::App(Io::StopSignIn(id))];
        }
        if let Some(index) = self.indexes.get(&current)
            && index.found.is_some()
            && let Some(&mailbox) = self.mailboxes.get(&(index.account, index.mailbox.clone()))
        {
            self.show_in_main(kit, mailbox);
            return Vec::new();
        }
        let window = kit.current_window();
        if window != self.main && Some(window) != self.sidebar_window() && Some(window) != self.mini
        {
            // A window of its own, as `:help`'s: it closes, as Vim's `q` would close it.
            return kit.run_ex(self, "close").unwrap_or_default();
        }
        let sidebar = self.sidebar.as_ref().map(|sidebar| sidebar.buffer);
        let elsewhere = !self.indexes.contains_key(&current) && Some(current) != sidebar;
        if elsewhere && (current != self.home || self.last_index.is_some()) {
            self.back_from(kit);
            return Vec::new();
        }
        self.ask_to_quit(kit)
    }

    /// Back from a buffer of muttrs' own (signing in, a command's output) to the Index last shown.
    fn back_from(&mut self, kit: &mut Kit) {
        if let Some(index) = self
            .last_index
            .filter(|index| self.indexes.contains_key(index))
        {
            self.show_in_main(kit, index);
        } else {
            let main = self.main_window(kit);
            kit.focus_window(main);
            kit.show_buffer(self.home);
        }
    }

    // --- The statusline. ---

    /// What the statusline says of the Accounts' Tasks, once one's worth saying: a spinner, the Task being done (or
    /// waiting), how long it's taken once that's a while, and how many more are asked.
    fn tasks_said(&self) -> Option<String> {
        let now = Instant::now();
        let (account, said) = self
            .accounts
            .iter()
            .enumerate()
            .find_map(|(account, state)| Some((account, state.tasks.said(now)?)))?;
        let turned = now.duration_since(self.started).as_millis() / SPIN.as_millis();
        let frame = SPINNER[usize::try_from(turned).unwrap_or(0) % SPINNER.len()];
        let what = if self.accounts.len() == 1 {
            said.what.to_owned()
        } else {
            format!("{}: {}", self.accounts[account].name, said.what)
        };
        let mut text = format!("{frame} {}", index::truncate(&what, 48));
        if let Some(since) = said.since
            && now.saturating_duration_since(since) >= TAKING_A_WHILE
        {
            let seconds = now.saturating_duration_since(since).as_secs();
            let _ = write!(text, " {seconds}s");
        }
        let more = self
            .accounts
            .iter()
            .map(|state| state.tasks.len())
            .sum::<usize>()
            .saturating_sub(1);
        if more > 0 {
            let _ = write!(text, "  {more} more");
        }
        Some(text)
    }

    /// What the Pager in the current window shows: HTML, Plain or Source.
    fn showing(&self, kit: &Kit) -> Option<String> {
        let pager = self.pagers.get(&kit.current_buffer())?;
        Some(
            match pager.showing() {
                pager::Body::Html => "HTML",
                pager::Body::Plain => "Plain",
                pager::Body::Source => "Source",
            }
            .to_owned(),
        )
    }

    // --- Signing in again. ---

    /// Signs in again to the Account called `name`, or else the one shown, in the browser.
    pub(crate) fn login(&mut self, kit: &mut Kit, name: &str) -> Result<Effects, String> {
        let account = if name.is_empty() {
            self.account_shown(kit)
        } else {
            self.accounts
                .iter()
                .position(|state| state.name == name)
                .ok_or_else(|| format!("No account called {name}"))?
        };
        let state = self
            .accounts
            .get(account)
            .ok_or_else(|| "There are no accounts".to_owned())?;
        let name = state.name.clone();
        crate::login::account(&self.config, &name).map_err(|error| format!("{error:#}"))?;
        let id = self.next_sign_in;
        self.next_sign_in += 1;
        let mut effects = Vec::new();
        if let Some(before) = self.signing_in.take() {
            effects.push(Effect::App(Io::StopSignIn(before.id)));
        }
        let buffer = kit
            .find_buffer("login")
            .unwrap_or_else(|| kit.add_buffer("login", Document::new(), false));
        *kit.buffer_mut(buffer).doc_mut() = sign_in_text(&name, None, None);
        self.signing_in = Some(SigningIn {
            id,
            account,
            buffer,
        });
        self.show_in_main(kit, buffer);
        effects.push(Effect::App(Io::SignIn { id, account: name }));
        Ok(effects)
    }

    /// Takes in how the sign-in `id` goes, if it's the one under way.
    fn signing_in(&mut self, kit: &mut Kit, id: u64, progress: Progress) -> Effects {
        let Some(signing_in) = self
            .signing_in
            .as_ref()
            .filter(|signing_in| signing_in.id == id)
        else {
            return Vec::new();
        };
        let (buffer, account) = (signing_in.buffer, signing_in.account);
        let name = self
            .accounts
            .get(account)
            .map_or("", |state| state.name.as_str())
            .to_owned();
        match progress {
            Progress::Started(url) => {
                *kit.buffer_mut(buffer).doc_mut() = sign_in_text(&name, Some(&url), None);
                Self::ask_for_address(kit, id);
                Vec::new()
            }
            Progress::Rejected(why) => {
                kit.error(why);
                Self::ask_for_address(kit, id);
                Vec::new()
            }
            Progress::Done(signed_in) => {
                self.signing_in = None;
                match &signed_in {
                    Ok(()) => {
                        if let Some(state) = self.accounts.get_mut(account) {
                            state.ask(client::Request::Refresh);
                        }
                        kit.info(format!("{name}: signed in"));
                    }
                    Err(why) => kit.error(format!("{name}: signing in failed: {why}")),
                }
                if kit.current_buffer() == buffer {
                    self.back_from(kit);
                }
                Vec::new()
            }
        }
    }

    /// Asks for the address the browser ended at, for the sign-in `id`, unless something else is asked.
    fn ask_for_address(kit: &mut Kit, id: u64) {
        kit.prompt(
            "Paste the address the browser ended at (Esc if it comes back by itself): ",
            false,
            move |muttrs: &mut Self, _, address| {
                if muttrs
                    .signing_in
                    .as_ref()
                    .is_some_and(|signing_in| signing_in.id == id)
                    && !address.trim().is_empty()
                {
                    vec![Effect::App(Io::Paste {
                        id,
                        address: address.trim().to_owned(),
                    })]
                } else {
                    Vec::new()
                }
            },
        );
    }
}

/// The text of the buffer saying how to sign in to the Account `name`: at `url` once the browser's opened there.
fn sign_in_text(name: &str, url: Option<&str>, _rejected: Option<&str>) -> Document {
    let mut lines = vec![Line::Text(vec![Span::new(
        format!("Signing in to {name}"),
        hl::SUBJECT,
    )])];
    lines.push(Line::text(""));
    match url {
        None => lines.push(Line::Text(vec![Span::new("Opening the browser…", hl::DIM)])),
        Some(url) => {
            lines.push(Line::text(
                "Sign in in the browser. Once it's done, it comes back to muttrs by itself, unless the browser",
            ));
            lines.push(Line::text(
                "is on another machine: then paste the address it ended at, from its address bar, at the prompt.",
            ));
            lines.push(Line::text(""));
            lines.push(Line::Text(vec![Span::new(
                "The page to sign in at (<CR> opens it, yy copies it):",
                hl::DIM,
            )]));
            lines.push(Line::Text(vec![
                Span::new(url, wcgw_kit::hl::Group::MARKUP_LINK)
                    .with_target(Target::Link(url.to_owned())),
            ]));
            lines.push(Line::text(""));
            lines.push(Line::Text(vec![Span::new(
                ":login again asks for the address again; q gives up.",
                hl::DIM,
            )]));
        }
    }
    let mut doc = Document::new();
    doc.push_back("login", lines);
    doc
}

/// What the first buffer says while muttrs connects.
fn connecting() -> Document {
    let mut doc = Document::new();
    doc.push_back(
        "connecting",
        vec![Line::Text(vec![Span::new(" Connecting…", hl::DIM)])],
    );
    doc
}

/// The sidebar's width on a screen `width` cells wide.
fn sidebar_width(width: u16) -> u16 {
    (width / 4).clamp(18, 32).min(width / 2)
}

/// A Message's subject as a buffer's name: short, and something when it's empty.
fn truncate_name(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        "(no subject)".to_owned()
    } else {
        index::truncate(subject, 60)
    }
}

impl wcgw_kit::App for Muttrs {
    type Target = Target;
    type Msg = Msg;
    type Effect = Io;

    fn update(&mut self, kit: &mut Kit, msg: Msg) -> Effects {
        let mut effects = match msg {
            Msg::Account(account, update) => self.on_account(kit, account, update),
            Msg::Read(pager) => {
                if self.reading == Some(pager) {
                    self.reading = None;
                    let opened = self
                        .pagers
                        .values()
                        .find(|shown| shown.id() == pager)
                        .and_then(|shown| shown.opened.clone());
                    if let Some(opened) = opened {
                        self.mark_seen(&opened);
                    }
                }
                Vec::new()
            }
            Msg::Tick => {
                self.ticking = false;
                Vec::new()
            }
            Msg::Fetched { source, pixels } => {
                for (&buffer, pager) in &mut self.pagers {
                    if pager.fetched(&source, pixels) {
                        self.dirty.insert(buffer);
                    }
                }
                Vec::new()
            }
            Msg::Voight(said) => self.voight_said(kit, said),
            Msg::SignIn(id, progress) => self.signing_in(kit, id, progress),
            Msg::Unsubscribed(said) | Msg::Said(said) => {
                match said {
                    Ok(said) => kit.info(said),
                    Err(said) => kit.error(said),
                }
                Vec::new()
            }
            Msg::Queried(asked, found) => {
                if self.querying.as_deref() == Some(asked.as_str()) {
                    self.querying = None;
                }
                self.queried = Some((asked, found));
                Vec::new()
            }
            Msg::Contacts(account, contacts) => {
                match contacts {
                    Ok(contacts) => self.addresses.add_contacts(contacts),
                    Err(why) => kit.error(format!("{account}'s address book: {why}")),
                }
                Vec::new()
            }
        };
        effects.extend(self.settle(kit));
        effects
    }

    fn start(&mut self, kit: &mut Kit) -> Effects {
        let (lines, ignored) = self.config.kit_settings();
        let mut effects = Vec::new();
        for line in lines {
            match kit.run_ex(self, &line) {
                Ok(more) => effects.extend(more),
                Err(error) => kit.error(format!("config.toml: {error}")),
            }
        }
        for said in ignored {
            kit.warn(format!("config.toml: {said}"));
        }
        self.everything_dirty();
        effects.extend(self.settle(kit));
        effects
    }

    fn open(&mut self, kit: &mut Kit, target: Target) -> Result<Effects, String> {
        let mut effects = match target {
            Target::Account(account) => {
                if let Some(state) = self.accounts.get_mut(account) {
                    state.ask(client::Request::Refresh);
                }
                Vec::new()
            }
            Target::Mailbox { account, name } => {
                self.open_mailbox(kit, account, &name);
                Vec::new()
            }
            Target::Message(opened) => {
                let from = self
                    .mailboxes
                    .get(&(opened.account, opened.mailbox.clone()))
                    .copied()
                    .filter(|_| !self.indexes.contains_key(&kit.current_buffer()))
                    .or(Some(kit.current_buffer()));
                self.open_message(kit, opened, from, None);
                Vec::new()
            }
            Target::Link(url) => vec![Effect::OpenUrl(url)],
            Target::Draft => self.edit_draft(),
            Target::Move { targets, to } => self.move_to(kit, targets, &to),
            Target::Label { targets, label } => {
                // Picked otherwise than by `<CR>`, as by a click: the list opens again, to add or take off more.
                self.toggle_label(&targets, label);
                let picker = self.labels_picker(&targets);
                kit.open_picker(self, picker)
            }
            Target::Found {
                account,
                query,
                mailbox,
            } => {
                self.open_mailbox(kit, account, &mailbox);
                if let Some(state) = self.accounts.get_mut(account) {
                    state.ask(client::Request::Search { mailbox, query });
                }
                Vec::new()
            }
            Target::Attachment { pager, part } => self.attachment(pager, part, false)?,
        };
        effects.extend(self.settle(kit));
        Ok(effects)
    }

    /// `:e {name}`: the Mailbox of that name, of the Account shown.
    fn edit(&mut self, kit: &mut Kit, name: &str) -> Result<Effects, String> {
        self.change_mailbox(kit, name)?;
        Ok(self.settle(kit))
    }

    fn edit_names(&self, kit: &Kit) -> Vec<String> {
        let account = self.account_shown(kit);
        self.accounts.get(account).map_or_else(Vec::new, |state| {
            state
                .mailboxes
                .iter()
                .filter(|mailbox| mailbox.selectable)
                .map(|mailbox| mailbox.path.join("/"))
                .collect()
        })
    }

    /// `<C-l>`, `:e!`: asks every Account what's new.
    fn reload(&mut self, kit: &mut Kit, _buffer: BufferId) -> Option<Effects> {
        self.refresh();
        Some(self.settle(kit))
    }

    fn shown(&mut self, kit: &mut Kit, buffer: BufferId) -> Effects {
        if self.pagers.contains_key(&buffer) {
            self.kept(kit, buffer);
        }
        if self.indexes.contains_key(&buffer) {
            self.last_index = Some(buffer);
        }
        if let Some(sidebar) = &self.sidebar {
            self.dirty.insert(sidebar.buffer);
        }
        Vec::new()
    }

    fn keyed(&mut self, kit: &mut Kit) -> Effects {
        self.leaving = match self.leaving {
            Leaving::Asked => Leaving::Waiting,
            Leaving::Waiting | Leaving::No => Leaving::No,
        };
        let current = kit.current_buffer();
        let mut effects = Vec::new();
        if self.indexes.contains_key(&current) {
            let near_end = kit.cursor_near_end();
            effects.extend(self.more_if_wanted(current, near_end));
        }
        if let Some(query) = self.to_query.borrow_mut().take()
            && self.querying.as_ref() != Some(&query)
            && let Some(command) = self.config.query_command.clone()
        {
            self.querying = Some(query.clone());
            effects.push(Effect::App(Io::Query {
                command,
                typed: query,
            }));
        }
        effects.extend(self.settle(kit));
        effects
    }

    /// `!` or `|` ran with the terminal: the kit has said how it went, if it went wrong.
    fn shelled(&mut self, kit: &mut Kit, _status: Result<i32, String>) -> Effects {
        self.settle(kit)
    }

    fn edited(&mut self, kit: &mut Kit, saved: Result<String, String>) -> Effects {
        let mut effects = self.draft_edited(kit, saved);
        effects.extend(self.settle(kit));
        effects
    }

    fn quit(&mut self, kit: &mut Kit) -> wcgw_kit::Quitting {
        self.leave(kit)
    }

    fn resized(&mut self, kit: &mut Kit, width: u16, _height: u16) -> Effects {
        if let Some(sidebar) = &mut self.sidebar {
            sidebar.width = sidebar_width(width);
            if let Some(window) = sidebar.window {
                kit.set_window_size(window, Size::Cells(sidebar.width));
            }
        }
        self.everything_dirty();
        self.settle(kit)
    }
}
