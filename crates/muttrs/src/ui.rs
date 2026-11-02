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
use muttrs_mail::Message;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr as _;

use crate::config::Config;
use crate::icons::Icons;
use crate::overlay::{self, Outcome, Overlay, Verb};
use crate::pager::{self, Pager, Request};
use crate::pictures::{self, Pictures};
use crate::remote::Fetcher;
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
}

/// A word for the user at the foot of the screen, until the next key.
struct Notice {
    text: String,
    error: bool,
}

pub(crate) struct App {
    config: Config,
    theme: Theme,
    icons: Icons,
    pictures: Pictures,
    fetcher: Option<Fetcher>,
    events: mpsc::UnboundedSender<Event>,
    pager: Option<Pager>,
    overlay: Option<Overlay>,
    prompt: Option<Prompt>,
    notice: Option<Notice>,
    next_pager: u64,
    quit: bool,
}

impl App {
    fn new(setup: Setup, events: mpsc::UnboundedSender<Event>) -> Self {
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
            pager: None,
            overlay: None,
            prompt: None,
            notice: None,
            next_pager: 1,
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
        }
        Ok(())
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
        if self.pager.is_some() {
            self.pager_key(key);
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
            _ => {}
        }
    }

    fn close_pager(&mut self) {
        self.pager = None;
        self.pictures.keep(None, |_| false);
        // Standing alone, as `muttrs view` opens it, the Pager is all there is.
        self.quit = true;
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
                        }
                    }
                    PromptKind::Search => {}
                    PromptKind::Command => self.command(text.trim()),
                }
            }
            _ => {}
        }
    }

    fn command(&mut self, command: &str) {
        match command {
            "" => {}
            "q" | "quit" | "q!" | "qa" => self.quit = true,
            "help" | "h" => self.help(),
            _ => self.say(&format!("Not a command: {command}"), true),
        }
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
                ("Anywhere", &[("?", "these keys"), (":q q", "quit")]),
            ],
        ));
    }

    fn chosen(&mut self, kind: overlay::Kind, index: usize, verb: Verb) {
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
            overlay::Kind::Help => {}
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
        if let Some(pager) = &mut self.pager {
            let mut cx = pager::Cx {
                theme: &self.theme,
                icons: &self.icons,
                pictures: &mut self.pictures,
                pager_width: self.config.pager_width,
                max_picture_rows: self.config.max_picture_rows,
                pixel_ratio: self.config.pixel_ratio,
            };
            pager.draw(body, buffer, &mut cx);
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.draw(body, buffer, &self.theme);
        }
        self.draw_status(status, frame);
    }

    fn draw_status(&self, area: Rect, frame: &mut Frame<'_>) {
        let buffer = frame.buffer_mut();
        buffer.set_style(area, self.theme.status);
        if let Some(prompt) = &self.prompt {
            let lead = match prompt.kind {
                PromptKind::Search => "/",
                PromptKind::Command => ":",
            };
            let line = Line::from(vec![Span::raw(lead), Span::raw(prompt.text.clone())]);
            buffer.set_line(area.x, area.y, &line, area.width);
            buffer.set_style(area, Style::new());
            let cursor = u16::try_from(1 + prompt.text.width()).unwrap_or(area.width);
            frame.set_cursor_position((area.x + cursor.min(area.width.saturating_sub(1)), area.y));
            return;
        }
        let mut right = Vec::new();
        if let Some(pager) = &self.pager {
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
            (None, Some(pager)) => Span::raw(format!(" {}", pager.message().subject)),
            (None, None) => Span::raw(" muttrs"),
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
    let (events, mut received) = mpsc::unbounded_channel();
    let kitty = setup.protocol == Some(Protocol::Kitty);
    let mut app = App::new(setup, events);
    app.open(Arc::new(message));

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
