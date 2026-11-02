//! The Pager: one Message, its headers then its body as rendered, with its Pictures, scrolled and searched.

use std::sync::Arc;

use jiff::tz::TimeZone;
use muttrs_mail::{Address, Message, Part};
use muttrs_render::{Document, ImageSource, Link, Options, PictureSlot, TextFormat};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr as _;

use crate::config::Prefer;
use crate::icons::Icons;
use crate::pictures::{self, Pictures};
use crate::theme::Theme;

/// Which of a Message's bodies shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Body {
    Html,
    Plain,
}

/// Where an image of the HTML body stands.
#[derive(Clone, Debug)]
enum Image {
    /// Remote Content, not loaded.
    Blocked,
    Fetching,
    Ready {
        bytes: Arc<Vec<u8>>,
        pixels: (u32, u32),
    },
    /// Not an image muttrs can draw, or not there: a `cid:` naming no Part, a fetch that failed.
    Broken,
}

impl Image {
    fn from_bytes(bytes: Vec<u8>) -> Self {
        match muttrs_gfx::dimensions(&bytes) {
            Ok(pixels) => Self::Ready {
                bytes: Arc::new(bytes),
                pixels,
            },
            Err(_) => Self::Broken,
        }
    }
}

/// The Pager's lines for one width: the headers, then the body.
struct Laid {
    width: u16,
    lines: Vec<Line<'static>>,
    /// Where Pictures go, their lines counted from the top of the headers.
    slots: Vec<PictureSlot>,
}

/// A search of the Pager's lines.
#[derive(Debug)]
struct Search {
    query: String,
    matches: Vec<Found>,
    /// The match the search is at.
    current: usize,
}

/// Where a search found its text: a line, and the cells from `start` to before `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Found {
    line: usize,
    start: u16,
    end: u16,
}

/// What the Pager asks of what holds it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// Nothing: the Pager handled it, or it wasn't for the Pager.
    None,
    /// Fetch these images' Remote Content: their indices and URLs.
    Fetch(Vec<(usize, String)>),
}

pub(crate) struct Pager {
    /// Tells this opening of a Message from any other, for its Pictures.
    id: u64,
    message: Arc<Message>,
    html: Option<Document>,
    plain: Option<Document>,
    showing: Body,
    /// The states of the HTML body's images, by index.
    images: Vec<Image>,
    remote_allowed: bool,
    laid: Option<Laid>,
    scroll: usize,
    /// How many lines the Pager last had to draw on.
    height: usize,
    search: Option<Search>,
}

impl Pager {
    /// The Pager on `message`, showing the body `prefer` says when it has both, and its Remote Content if
    /// `remote_allowed`; `id` tells it from other openings, for their Pictures.
    pub(crate) fn new(
        id: u64,
        message: Arc<Message>,
        theme: &Theme,
        prefer: Prefer,
        remote_allowed: bool,
    ) -> Self {
        let html = message
            .html()
            .map(|html| Document::html(&html, &theme.body));
        let plain = message.plain().map(|(text, format)| {
            let format = match format {
                muttrs_mail::TextFormat::Fixed => TextFormat::Fixed,
                muttrs_mail::TextFormat::Flowed { delete_space } => {
                    TextFormat::Flowed { delete_space }
                }
            };
            Document::plain(&text, format, &theme.body)
        });
        let showing = match (&html, &plain, prefer) {
            (Some(_), Some(_), Prefer::Html) | (Some(_), None, _) => Body::Html,
            _ => Body::Plain,
        };
        let images = html.as_ref().map_or_else(Vec::new, |html| {
            html.images()
                .iter()
                .map(|image| match &image.source {
                    ImageSource::Cid(id) => message
                        .by_content_id(id)
                        .map_or(Image::Broken, |part| Image::from_bytes(message.bytes(part))),
                    ImageSource::Data(bytes) => Image::from_bytes(bytes.clone()),
                    ImageSource::Remote(_) => Image::Blocked,
                })
                .collect()
        });
        Self {
            id,
            message,
            html,
            plain,
            showing,
            images,
            remote_allowed,
            laid: None,
            scroll: 0,
            height: 0,
            search: None,
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn message(&self) -> &Arc<Message> {
        &self.message
    }

    pub(crate) fn showing(&self) -> Body {
        self.showing
    }

    fn document(&self) -> Option<&Document> {
        match self.showing {
            Body::Html => self.html.as_ref(),
            Body::Plain => self.plain.as_ref(),
        }
    }

    /// The Links of the body shown.
    pub(crate) fn links(&self) -> &[Link] {
        self.document().map_or(&[], Document::links)
    }

    /// The Message's Attachments: its Parts but the Inline Images its HTML body shows.
    pub(crate) fn attachments(&self) -> Vec<&Part> {
        let shown: Vec<&str> = self
            .html
            .iter()
            .flat_map(muttrs_render::Document::images)
            .filter_map(|image| match &image.source {
                ImageSource::Cid(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        self.message
            .parts()
            .iter()
            .filter(|part| {
                !part
                    .content_id
                    .as_deref()
                    .is_some_and(|id| shown.iter().any(|shown| shown.eq_ignore_ascii_case(id)))
            })
            .collect()
    }

    /// How many images of the HTML body are Remote Content not loaded.
    fn blocked(&self) -> usize {
        self.images
            .iter()
            .filter(|image| matches!(image, Image::Blocked))
            .count()
    }

    /// Shows the other body, if there's another.
    pub(crate) fn toggle_body(&mut self) -> bool {
        let other = match self.showing {
            Body::Html if self.plain.is_some() => Body::Plain,
            Body::Plain if self.html.is_some() => Body::Html,
            _ => return false,
        };
        self.showing = other;
        self.relayout();
        self.scroll = 0;
        true
    }

    /// Allows Remote Content, returning the images to fetch.
    pub(crate) fn allow_remote(&mut self) -> Request {
        self.remote_allowed = true;
        self.remote_to_fetch()
    }

    /// The Remote Content to fetch, if it's allowed: what's blocked, now fetching.
    pub(crate) fn remote_to_fetch(&mut self) -> Request {
        if !self.remote_allowed {
            return Request::None;
        }
        let Some(html) = &self.html else {
            return Request::None;
        };
        let mut fetch = Vec::new();
        for (index, image) in self.images.iter_mut().enumerate() {
            if let (Image::Blocked, ImageSource::Remote(url)) =
                (&image, &html.images()[index].source)
            {
                fetch.push((index, url.clone()));
                *image = Image::Fetching;
            }
        }
        if fetch.is_empty() {
            Request::None
        } else {
            self.relayout();
            Request::Fetch(fetch)
        }
    }

    /// Takes the bytes fetched for image `index`, or `None` if they couldn't be.
    pub(crate) fn fetched(&mut self, index: usize, bytes: Option<Vec<u8>>) {
        if let Some(image) = self.images.get_mut(index) {
            *image = bytes.map_or(Image::Broken, Image::from_bytes);
            self.relayout();
        }
    }

    fn relayout(&mut self) {
        self.laid = None;
    }

    /// Lays the Pager out again, for cells of another size: its Pictures take other rows.
    pub(crate) fn cell_changed(&mut self) {
        self.relayout();
    }

    /// Lays the Pager out `width` cells wide, unless it is already.
    fn lay_out(&mut self, width: u16, cx: &Cx<'_>) {
        if self.laid.as_ref().is_some_and(|laid| laid.width == width) {
            return;
        }
        let mut lines = self.headers(width, cx.theme, cx.icons);
        let header_lines = lines.len();
        let sizes: Vec<Option<(u32, u32)>> = if self.showing == Body::Html && cx.pictures.enabled()
        {
            self.images
                .iter()
                .map(|image| match image {
                    Image::Ready { pixels, .. } => Some(*pixels),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let cell = cx.pictures.cell();
        let rendering = self.document().map(|document| {
            document.render(&Options {
                width,
                cell: muttrs_render::CellSize {
                    width: cell.width,
                    height: cell.height,
                },
                pixel_ratio: cx.pixel_ratio.unwrap_or_else(|| {
                    muttrs_render::CellSize {
                        width: cell.width,
                        height: cell.height,
                    }
                    .likely_pixel_ratio()
                }),
                max_picture_rows: cx.max_picture_rows,
                pictures: &sizes,
            })
        });
        let mut slots = Vec::new();
        match rendering {
            Some(rendering) => {
                lines.extend(rendering.lines);
                slots = rendering
                    .pictures
                    .into_iter()
                    .map(|slot| PictureSlot {
                        line: slot.line + header_lines,
                        ..slot
                    })
                    .collect();
            }
            None => lines.push(Line::styled(
                "This Message has no body to show.",
                cx.theme.dim,
            )),
        }
        self.laid = Some(Laid {
            width,
            lines,
            slots,
        });
        if let Some(search) = self.search.take() {
            self.search_for(search.query);
        }
    }

    /// The headers, laid out `width` cells wide.
    fn headers(&self, width: u16, theme: &Theme, icons: &Icons) -> Vec<Line<'static>> {
        let message = &self.message;
        let mut lines = Vec::new();
        let label = |name: &str| Span::styled(format!("{name:<9}"), theme.dim);
        let addresses = |list: &[Address]| {
            list.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        for (name, list) in [
            ("From", &message.from),
            ("To", &message.to),
            ("Cc", &message.cc),
        ] {
            if !list.is_empty() {
                lines.extend(wrap_header(
                    &label(name),
                    &addresses(list),
                    Style::new(),
                    width,
                ));
            }
        }
        if let Some(date) = message.date {
            let local = date.to_zoned(TimeZone::system());
            lines.push(Line::from(vec![
                label("Date"),
                Span::raw(local.strftime("%a %-d %b %Y, %H:%M").to_string()),
            ]));
        }
        let subject = if message.subject.is_empty() {
            "(no subject)"
        } else {
            &message.subject
        };
        lines.extend(wrap_header(
            &label("Subject"),
            subject,
            theme.subject,
            width,
        ));

        let attachments = self.attachments();
        if !attachments.is_empty() {
            let listed = attachments
                .iter()
                .map(|part| format!("{} ({})", part.file_name(), size(part.size)))
                .collect::<Vec<_>>()
                .join(" · ");
            let mark = Span::styled(
                format!("{:<9}", icons.attachment.trim_end()),
                theme.attachment,
            );
            lines.extend(wrap_header(&mark, &listed, theme.attachment, width));
        }
        let blocked = self.blocked();
        if blocked > 0 && self.showing == Body::Html {
            let images = if blocked == 1 { "image" } else { "images" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}{blocked} remote {images} not loaded: ", icons.blocked),
                    theme.banner,
                ),
                Span::styled("I", theme.keys),
                Span::styled(" loads them", theme.banner),
            ]));
        }
        lines.push(Line::styled("─".repeat(usize::from(width)), theme.dim));
        lines
    }

    /// Searches the lines for `query`, ignoring case, going to its first match from the top of what shows.
    pub(crate) fn search_for(&mut self, query: String) {
        let Some(laid) = &self.laid else {
            self.search = Some(Search {
                query,
                matches: Vec::new(),
                current: 0,
            });
            return;
        };
        let matches = find(&laid.lines, &query);
        let current = matches
            .iter()
            .position(|found| found.line >= self.scroll)
            .unwrap_or(0);
        self.search = Some(Search {
            query,
            matches,
            current,
        });
        self.show_current_match();
    }

    /// Goes to the next match, or the previous one, wrapping around.
    pub(crate) fn next_match(&mut self, forward: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        let count = search.matches.len();
        if count == 0 {
            return;
        }
        search.current = if forward {
            (search.current + 1) % count
        } else {
            (search.current + count - 1) % count
        };
        self.show_current_match();
    }

    pub(crate) fn clear_search(&mut self) -> bool {
        self.search.take().is_some()
    }

    /// The query searched for, and how many matches and which it's at.
    pub(crate) fn search_status(&self) -> Option<(String, usize, usize)> {
        let search = self.search.as_ref()?;
        Some((
            search.query.clone(),
            search.current + 1,
            search.matches.len(),
        ))
    }

    fn show_current_match(&mut self) {
        let Some(found) = self
            .search
            .as_ref()
            .and_then(|search| search.matches.get(search.current))
        else {
            return;
        };
        if found.line < self.scroll || found.line >= self.scroll + self.height.max(1) {
            self.scroll = found.line.saturating_sub(self.height / 3);
        }
    }

    /// Scrolls by `lines`, down if positive.
    pub(crate) fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines);
        self.clamp();
    }

    /// Scrolls by a fraction of what shows: `pages` pages, down if positive.
    pub(crate) fn scroll_pages(&mut self, pages: f32) {
        // A page is at most a few thousand lines; rounding is what's wanted.
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let lines = (self.height.max(2) as f32 * pages).round() as isize;
        self.scroll_by(lines);
    }

    pub(crate) fn top(&mut self) {
        self.scroll = 0;
    }

    pub(crate) fn bottom(&mut self) {
        self.scroll = usize::MAX;
        self.clamp();
    }

    fn line_count(&self) -> usize {
        self.laid.as_ref().map_or(0, |laid| laid.lines.len())
    }

    fn clamp(&mut self) {
        self.scroll = self
            .scroll
            .min(self.line_count().saturating_sub(self.height.max(1)));
    }

    /// How far down the Pager is scrolled, as "Top", "Bot", "All" or a percentage, as vim says.
    pub(crate) fn position(&self) -> String {
        let total = self.line_count();
        if total <= self.height {
            "All".into()
        } else if self.scroll == 0 {
            "Top".into()
        } else if self.scroll + self.height >= total {
            "Bot".into()
        } else {
            format!("{}%", self.scroll * 100 / (total - self.height))
        }
    }

    /// Draws the Pager in `area` of `buffer`.
    pub(crate) fn draw(&mut self, area: Rect, buffer: &mut Buffer, cx: &mut Cx<'_>) {
        if area.width < 4 || area.height == 0 {
            return;
        }
        let width = (area.width - 2).min(cx.pager_width);
        self.lay_out(width, cx);
        self.height = usize::from(area.height);
        self.clamp();
        let (x, y) = (area.x + 1, area.y);
        let Some(laid) = &self.laid else { return };
        for (row, line) in laid
            .lines
            .iter()
            .skip(self.scroll)
            .take(self.height)
            .enumerate()
        {
            // Fewer rows than the area's height, a u16.
            let row = u16::try_from(row).unwrap_or(u16::MAX);
            buffer.set_line(x, y + row, line, width);
        }

        // Pictures, over the blank rows left for them.
        let id = self.id;
        let wanted: Vec<pictures::Key> = laid
            .slots
            .iter()
            .map(|slot| pictures::Key {
                message: id,
                image: slot.image,
                cols: slot.cols,
                rows: slot.rows,
            })
            .collect();
        cx.pictures.keep(Some(id), |key| wanted.contains(key));
        for (slot, key) in laid.slots.iter().zip(&wanted) {
            let visible = (0..slot.rows).filter_map(|row| {
                let line = slot.line + usize::from(row);
                let on_screen = line
                    .checked_sub(self.scroll)
                    .filter(|&on| on < self.height)?;
                Some((row, u16::try_from(on_screen).ok()?))
            });
            let visible: Vec<(u16, u16)> = visible.collect();
            if visible.is_empty() {
                continue;
            }
            let bytes = || match self.images.get(slot.image) {
                Some(Image::Ready { bytes, .. }) => Some(Arc::clone(bytes)),
                _ => None,
            };
            if let Some(picture) = cx.pictures.get(*key, bytes) {
                for (row, on_screen) in visible {
                    let spans = picture.row(row);
                    buffer.set_line(
                        x + slot.column,
                        y + on_screen,
                        &Line::from(spans),
                        slot.cols,
                    );
                }
            } else {
                // Until it's ready: where it goes, and that it's coming.
                let (_, on_screen) = visible[0];
                let waiting = Line::styled(format!("{}…", cx.icons.image), cx.theme.dim);
                buffer.set_line(x + slot.column, y + on_screen, &waiting, slot.cols);
            }
        }

        if let Some(search) = &self.search {
            for (index, found) in search.matches.iter().enumerate() {
                let Some(row) = found
                    .line
                    .checked_sub(self.scroll)
                    .filter(|&row| row < self.height)
                else {
                    continue;
                };
                let style = if index == search.current {
                    cx.theme.found_current
                } else {
                    cx.theme.found
                };
                let row = u16::try_from(row).unwrap_or(u16::MAX);
                let start = found.start.min(width);
                let end = found.end.min(width);
                buffer.set_style(Rect::new(x + start, y + row, end - start, 1), style);
            }
        }
    }
}

/// What drawing the Pager takes from what holds it.
pub(crate) struct Cx<'a> {
    pub(crate) theme: &'a Theme,
    pub(crate) icons: &'a Icons,
    pub(crate) pictures: &'a mut Pictures,
    pub(crate) pager_width: u16,
    pub(crate) max_picture_rows: u16,
    /// The terminal's pixels to a CSS pixel, if the configuration says; else guessed from the cells' size.
    pub(crate) pixel_ratio: Option<f32>,
}

/// A header's lines: its `label`, then `value` wrapped to `width`, indented under the label.
fn wrap_header(label: &Span<'static>, value: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let indent = label.content.width();
    let room = usize::from(width).saturating_sub(indent).max(10);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in value.split(' ') {
        if !line.is_empty() && line.width() + 1 + word.width() > room {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let first = if index == 0 {
                label.clone()
            } else {
                Span::raw(" ".repeat(indent))
            };
            Line::from(vec![first, Span::styled(text, style)])
        })
        .collect()
}

/// A size in bytes, as people read it.
pub(crate) fn size(bytes: usize) -> String {
    // Sizes of mail are far below where f64 loses precision.
    #[allow(clippy::cast_precision_loss)]
    let bytes_f = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.0} KB", bytes_f / 1024.0)
    } else {
        format!("{:.1} MB", bytes_f / (1024.0 * 1024.0))
    }
}

/// Where `query` is in `lines`, ignoring case, in cells.
fn find(lines: &[Line<'_>], query: &str) -> Vec<Found> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        // Each character, lower case, with the cell it starts at.
        let mut chars: Vec<(char, u16)> = Vec::new();
        let mut column = 0u16;
        for span in &line.spans {
            for character in span.content.chars() {
                for lower in character.to_lowercase() {
                    chars.push((lower, column));
                }
                let width = unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
                column = column.saturating_add(u16::try_from(width).unwrap_or(0));
            }
        }
        let mut at = 0;
        while at + query.len() <= chars.len() {
            if chars[at..at + query.len()]
                .iter()
                .map(|(c, _)| c)
                .eq(query.iter())
            {
                let start = chars[at].1;
                let end = chars
                    .get(at + query.len())
                    .map_or(column, |(_, next)| *next);
                matches.push(Found {
                    line: index,
                    start,
                    end,
                });
                at += query.len();
            } else {
                at += 1;
            }
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_find_every_match_ignoring_case_in_cells() {
        let lines = vec![
            Line::from(vec![Span::raw("A café, "), Span::raw("CAFÉ!")]),
            Line::raw("none here"),
            Line::raw("日本 café"),
        ];
        assert_eq!(
            find(&lines, "Café"),
            [
                Found {
                    line: 0,
                    start: 2,
                    end: 6
                },
                Found {
                    line: 0,
                    start: 8,
                    end: 12
                },
                Found {
                    line: 2,
                    start: 5,
                    end: 9
                },
            ]
        );
    }

    #[test]
    fn long_headers_wrap_under_their_value() {
        let lines = wrap_header(
            &Span::raw("To       "),
            "a@example.com, b@example.com",
            Style::new(),
            25,
        );
        let text: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(text, ["To       a@example.com,", "         b@example.com"]);
    }

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(12_800), "12 KB");
        assert_eq!(size(3 * 1024 * 1024 + 1), "3.0 MB");
    }
}
