//! The Pager: one Message as a kit buffer, its headers then its body as rendered, with its Pictures, which the kit
//! draws as their lines come into view.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use jiff::tz::TimeZone;
use muttrs_mail::{Address, Message, Part};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};
use wcgw_kit::BufferId;
use wcgw_kit::doc::{BlockKey, Content};
use wcgw_kit_text::{CellSize, Document, ImageSource, Link, Options, Styles, TextFormat};

use crate::app::{Line, Opened, Span, Target};
use crate::assessment::{self, Window};
use crate::config::Prefer;
use crate::heatmap::{self, Heat};
use crate::hl::{self, Group};

/// Which of a Message's bodies shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Body {
    Html,
    Plain,
    /// The whole Message as it came, its headers and its body as written.
    Source,
}

/// Where an image of the HTML body stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Image {
    /// Remote Content, not loaded.
    Blocked,
    Fetching,
    /// Its bytes are in the [`Pictures`], and it's this many pixels.
    Ready((u32, u32)),
    /// Not an image muttrs can draw, or not there: a `cid:` naming no Part, a fetch that failed.
    Broken,
}

/// The bytes of the Pictures the Pagers show, by the source their lines name: shared with what gets them for the kit.
pub(crate) type Pictures = Arc<Mutex<HashMap<String, Arc<Vec<u8>>>>>;

/// The size, in pixels, of the image in `bytes`, if it's one muttrs draws.
pub(crate) fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// What the Pager draws with: the kit's styles for bodies and the Heatmap, its icons, the cells' size, and the
/// configuration's say on Pictures.
pub(crate) struct Look {
    pub(crate) heat: Heat,
    pub(crate) attachment: String,
    pub(crate) blocked: String,
    pub(crate) cell: CellSize,
    pub(crate) pixel_ratio: Option<f32>,
    pub(crate) max_picture_rows: u16,
    /// Whether Pictures are drawn at all.
    pub(crate) pictures: bool,
}

/// What a Pager wants done after a change.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Request {
    /// Nothing.
    None,
    /// Fetch these images' Remote Content: their sources and URLs.
    Fetch(Vec<(String, String)>),
}

pub(crate) struct Pager {
    /// Tells this opening of a Message from any other, for its Pictures.
    id: u64,
    message: Arc<Message>,
    /// Where it is, if it's an Account's.
    pub(crate) opened: Option<Opened>,
    /// The Index it was opened from, to go back to.
    pub(crate) from: Option<BufferId>,
    html: Option<Document>,
    plain: Option<Document>,
    /// The body shown, HTML or plain text.
    showing: Body,
    /// Whether every header shows, as written, rather than those people read.
    all_headers: bool,
    /// Whether the Message's source shows, in place of its headers and body.
    source: bool,
    /// The states of the HTML body's images, by index.
    images: Vec<Image>,
    remote_allowed: bool,
    /// The prose of the body shown, as voight assesses it.
    prose: Option<assessment::Key>,
}

impl Pager {
    /// The Pager on `message`, showing the body `prefer` says when it has both, drawn in `styles`, its Remote
    /// Content loaded if `remote_allowed`; `id` tells it from other openings, for their Pictures, whose bytes go in
    /// `pictures`.
    pub(crate) fn new(
        id: u64,
        message: Arc<Message>,
        styles: &Styles,
        prefer: Prefer,
        remote_allowed: bool,
        pictures: &Pictures,
    ) -> Self {
        let html = message.html().map(|html| Document::html(&html, styles));
        let plain = message.plain().map(|(text, format)| {
            let format = match format {
                muttrs_mail::TextFormat::Fixed => TextFormat::Fixed,
                muttrs_mail::TextFormat::Flowed { delete_space } => {
                    TextFormat::Flowed { delete_space }
                }
            };
            Document::plain(&text, format, styles)
        });
        let showing = match (&html, &plain, prefer) {
            (Some(_), Some(_), Prefer::Html) | (Some(_), None, _) => Body::Html,
            _ => Body::Plain,
        };
        let mut stored = pictures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let images = html.as_ref().map_or_else(Vec::new, |html| {
            html.images()
                .iter()
                .enumerate()
                .map(|(index, image)| {
                    let bytes = match &image.source {
                        ImageSource::Cid(cid) => {
                            message.by_content_id(cid).map(|part| message.bytes(part))
                        }
                        ImageSource::Data(bytes) => Some(bytes.clone()),
                        ImageSource::Remote(_) => return Image::Blocked,
                    };
                    match bytes.and_then(|bytes| Some((dimensions(&bytes)?, bytes))) {
                        Some((pixels, bytes)) => {
                            stored.insert(source(id, index), Arc::new(bytes));
                            Image::Ready(pixels)
                        }
                        None => Image::Broken,
                    }
                })
                .collect()
        });
        drop(stored);
        let mut pager = Self {
            id,
            message,
            opened: None,
            from: None,
            html,
            plain,
            showing,
            all_headers: false,
            source: false,
            images,
            remote_allowed,
            prose: None,
        };
        pager.prose = pager.prose_shown();
        pager
    }

    fn prose_shown(&self) -> Option<assessment::Key> {
        if self.source {
            return None;
        }
        self.document().map(|document| Arc::from(document.prose()))
    }

    /// The prose of the body shown, as voight assesses it; `None` without a body, or showing the source.
    pub(crate) fn prose(&self) -> Option<&assessment::Key> {
        self.prose.as_ref()
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn message(&self) -> &Arc<Message> {
        &self.message
    }

    /// What shows: a body, or the source.
    pub(crate) fn showing(&self) -> Body {
        if self.source {
            Body::Source
        } else {
            self.showing
        }
    }

    /// Shows every header, as written, or only those people read.
    pub(crate) fn toggle_headers(&mut self) {
        self.all_headers = !self.all_headers;
    }

    /// Shows the Message's source in place of its headers and body, or them again.
    pub(crate) fn toggle_source(&mut self) {
        self.source = !self.source;
        self.prose = self.prose_shown();
    }

    fn document(&self) -> Option<&Document> {
        match self.showing {
            Body::Html => self.html.as_ref(),
            Body::Plain | Body::Source => self.plain.as_ref(),
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
            .flat_map(Document::images)
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
        // From the source, back to the body it was.
        if self.source {
            self.toggle_source();
            return true;
        }
        let other = match self.showing {
            Body::Html if self.plain.is_some() => Body::Plain,
            Body::Plain if self.html.is_some() => Body::Html,
            _ => return false,
        };
        self.showing = other;
        self.prose = self.prose_shown();
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
                fetch.push((source(self.id, index), url.clone()));
                *image = Image::Fetching;
            }
        }
        if fetch.is_empty() {
            Request::None
        } else {
            Request::Fetch(fetch)
        }
    }

    /// Takes the size of the Remote Content fetched for the image at `source`, in the [`Pictures`] now, or `None`
    /// if it couldn't be had: whether it's one of this Pager's.
    pub(crate) fn fetched(&mut self, source: &str, pixels: Option<(u32, u32)>) -> bool {
        let Some(index) = index_of(self.id, source) else {
            return false;
        };
        let Some(image) = self.images.get_mut(index) else {
            return false;
        };
        *image = pixels.map_or(Image::Broken, Image::Ready);
        true
    }

    /// The Pager as blocks of a kit document, `width` cells wide: its headers, then its body, a Link's text and
    /// number opening it, each Picture a line of its own the kit draws it over; the prose shaded by `heat`, the
    /// windows of its Assessment, if given, once the Links are found (by their styles, which shading changes).
    pub(crate) fn blocks(
        &self,
        width: u16,
        look: &Look,
        heat: Option<&[Window]>,
    ) -> Vec<(BlockKey, Content<Target>)> {
        if self.source {
            return vec![(
                BlockKey::new("source"),
                Content::from(source_lines(&self.message.source(), width)),
            )];
        }
        let headers = Content::from(self.headers(width, look));
        let sizes: Vec<Option<(u32, u32)>> = if self.showing == Body::Html && look.pictures {
            self.images
                .iter()
                .map(|image| match image {
                    Image::Ready(pixels) => Some(*pixels),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        let body = match self.document() {
            Some(document) => {
                let rendering = document.render(&Options {
                    width,
                    cell: look.cell,
                    pixel_ratio: look
                        .pixel_ratio
                        .unwrap_or_else(|| look.cell.likely_pixel_ratio()),
                    max_picture_rows: look.max_picture_rows,
                    pictures: &sizes,
                });
                let mut lines = wcgw_kit_text::kit::linked_lines(
                    document,
                    &rendering,
                    |image| source(self.id, image),
                    |_, link| Some(Target::Link(link.url.clone())),
                );
                if let (Some(heat), Some(prose)) = (heat, &self.prose) {
                    heatmap::shade_lines(&mut lines, prose, heat, &look.heat);
                }
                lines
            }
            None => vec![Line::Text(vec![Span::new(
                "This Message has no body to show.",
                hl::DIM,
            )])],
        };
        vec![
            (BlockKey::new("headers"), headers),
            (BlockKey::new("body"), Content::from(body)),
        ]
    }

    /// The headers, laid out `width` cells wide, then a rule.
    fn headers(&self, width: u16, look: &Look) -> Vec<Line> {
        let message = &self.message;
        let mut lines = Vec::new();
        let rule = Line::Text(vec![Span::new("─".repeat(usize::from(width)), hl::DIM)]);
        if self.all_headers {
            for (name, value) in message.headers() {
                lines.extend(wrap_header(
                    &format!("{name}: "),
                    hl::DIM,
                    &value,
                    Group::NORMAL,
                    width,
                ));
            }
            lines.push(rule);
            return lines;
        }
        let label = |name: &str| format!("{name:<9}");
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
                    hl::DIM,
                    &addresses(list),
                    Group::NORMAL,
                    width,
                ));
            }
        }
        if let Some(date) = message.date {
            let local = date.to_zoned(TimeZone::system());
            lines.push(Line::Text(vec![
                Span::new(label("Date"), hl::DIM),
                Span::new(wcgw_kit::when::long(&local), Group::NORMAL),
            ]));
        }
        let subject = if message.subject.is_empty() {
            "(no subject)"
        } else {
            &message.subject
        };
        lines.extend(wrap_header(
            &label("Subject"),
            hl::DIM,
            subject,
            hl::SUBJECT,
            width,
        ));
        let attachments = self.attachments();
        if !attachments.is_empty() {
            let listed = attachments
                .iter()
                .map(|part| format!("{} ({})", part.file_name(), size(part.size)))
                .collect::<Vec<_>>()
                .join(" · ");
            let mark = format!("{:<9}", look.attachment.trim_end());
            lines.extend(wrap_header(
                &mark,
                hl::ATTACHMENT,
                &listed,
                hl::ATTACHMENT,
                width,
            ));
        }
        let blocked = self.blocked();
        if blocked > 0 && self.showing == Body::Html {
            let images = if blocked == 1 { "image" } else { "images" };
            lines.push(Line::Text(vec![Span::new(
                format!(
                    "{}{blocked} remote {images} not loaded: :remote (I) loads them",
                    look.blocked
                ),
                hl::BANNER,
            )]));
        }
        lines.push(rule);
        lines
    }
}

/// Where image `index` of the Pager `id` is, as its line names it.
fn source(id: u64, index: usize) -> String {
    format!("muttrs:{id}/{index}")
}

/// The index of the image at `source`, if it's one of the Pager `id`'s.
fn index_of(id: u64, source: &str) -> Option<usize> {
    source.strip_prefix(&format!("muttrs:{id}/"))?.parse().ok()
}

/// A header's lines: its `label`, then `value` wrapped to `width`, indented under the label.
fn wrap_header(
    label: &str,
    label_group: Group,
    value: &str,
    group: Group,
    width: u16,
) -> Vec<Line> {
    let indent = label.width();
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
                Span::new(label, label_group)
            } else {
                Span::new(" ".repeat(indent), Group::NORMAL)
            };
            Line::Text(vec![first, Span::new(text, group)])
        })
        .collect()
}

/// The lines of `source`, as written, each cut into `width` cells' worth where it's longer.
fn source_lines(source: &str, width: u16) -> Vec<Line> {
    let width = usize::from(width).max(1);
    let mut lines = Vec::new();
    for line in source.lines() {
        let mut piece = String::new();
        for character in line.chars() {
            let cells = character.width().unwrap_or(0);
            if piece.width() + cells > width {
                lines.push(Line::text(std::mem::take(&mut piece)));
            }
            // Tabs as a space: their width depends on where they fall.
            piece.push(if character == '\t' { ' ' } else { character });
        }
        lines.push(Line::text(piece));
    }
    lines
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        match line {
            Line::Text(spans) => spans.iter().map(|span| span.text.as_str()).collect(),
            Line::Image { source, .. } => format!("<{source}>"),
        }
    }

    #[test]
    fn the_source_shows_line_by_line_cut_to_the_width() {
        let lines: Vec<String> = source_lines("ab\r\ncdefg\r\n\r\n\tz", 3)
            .iter()
            .map(text)
            .collect();
        assert_eq!(lines, ["ab", "cde", "fg", "", " z"]);
    }

    #[test]
    fn long_headers_wrap_under_their_value() {
        let lines = wrap_header(
            "To       ",
            hl::DIM,
            "a@example.com, b@example.com",
            Group::NORMAL,
            25,
        );
        let lines: Vec<String> = lines.iter().map(text).collect();
        assert_eq!(lines, ["To       a@example.com,", "         b@example.com"]);
    }

    #[test]
    fn sizes() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(12_800), "12 KB");
        assert_eq!(size(3 * 1024 * 1024 + 1), "3.0 MB");
    }

    #[test]
    fn pictures_are_lines_of_their_own_named_by_the_pager_and_image() {
        assert_eq!(source(7, 2), "muttrs:7/2");
        assert_eq!(index_of(7, "muttrs:7/2"), Some(2));
        assert_eq!(index_of(8, "muttrs:7/2"), None);
    }
}
