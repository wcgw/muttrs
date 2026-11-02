//! A Message's body, HTML or plain text, laid out as styled terminal lines.
//!
//! A body is read once, into a [`Document`]: [`Document::html`] parses HTML and applies the CSS a terminal can show,
//! [`Document::plain`] reads plain text, quotes and all. [`Document::render`] then lays it out for a width, as a
//! [`Rendering`]: lines of ratatui [`Line`]s, and where to draw the Pictures of the images whose size is known. It's
//! cheap enough to do again whenever the width changes or an image arrives.
//!
//! Nothing here touches the terminal or the network: images are listed ([`Document::images`]) for the caller to
//! fetch and draw, and Links are numbered ([`Document::links`]) for the caller to open. See
//! `docs/adr/0001-own-html-layout.md` for why muttrs lays HTML out itself.

mod color;
mod css;
mod html;
mod ir;
mod layout;
mod plain;
mod table;

use ratatui_core::style::{Modifier, Style};
use ratatui_core::text::Line;

pub use crate::color::Rgb;
pub use crate::css::Length;
use crate::ir::Block;

/// The styles a body is drawn in, which the theme provides.
#[derive(Clone, Debug)]
pub struct Styles {
    /// Text, where nothing else applies.
    pub text: Style,
    /// The background authors' colours are judged against, to keep only those that read there (see
    /// [`Rgb`]); `None` draws every text in the theme's colours.
    pub page: Option<Rgb>,
    /// A Link's text.
    pub link: Style,
    /// The number after a Link, as `[3]`.
    pub link_number: Style,
    /// Headings.
    pub heading: Style,
    /// Code, inline and in `<pre>`.
    pub code: Style,
    /// `<mark>`ed text.
    pub mark: Style,
    /// The bar of a quote, by depth: the first for the outermost, cycling when quotes go deeper.
    pub quotes: Vec<Style>,
    /// The line introducing a quote, as "On Monday, Ann wrote:".
    pub attribution: Style,
    /// A plain text signature, below its `-- ` line.
    pub signature: Style,
    /// Rules, and the lines of tables.
    pub rule: Style,
    /// A table's header cells.
    pub table_header: Style,
    /// The description shown for an image that isn't drawn.
    pub image: Style,
    /// The characters drawn for structure.
    pub glyphs: Glyphs,
}

/// The characters a [`Rendering`] draws structure with.
#[derive(Clone, Debug)]
pub struct Glyphs {
    /// Before an image's description, where it isn't drawn: a Nerd Font icon, or a word.
    pub image: String,
    /// A quote's bar, one cell wide.
    pub quote: String,
    /// The bullets of unordered lists, by depth, cycling deeper.
    pub bullets: Vec<String>,
}

impl Default for Glyphs {
    fn default() -> Self {
        Self {
            image: "image: ".into(),
            quote: "▎".into(),
            bullets: vec!["•".into(), "◦".into(), "▪".into()],
        }
    }
}

impl Default for Styles {
    /// The terminal's own colours, with emphasis only.
    fn default() -> Self {
        let dim = Style::new().add_modifier(Modifier::DIM);
        Self {
            text: Style::new(),
            page: None,
            link: Style::new().add_modifier(Modifier::UNDERLINED),
            link_number: dim,
            heading: Style::new().add_modifier(Modifier::BOLD),
            code: Style::new(),
            mark: Style::new().add_modifier(Modifier::REVERSED),
            quotes: vec![dim],
            attribution: Style::new().add_modifier(Modifier::ITALIC),
            signature: dim,
            rule: dim,
            table_header: Style::new().add_modifier(Modifier::BOLD),
            image: dim.add_modifier(Modifier::ITALIC),
            glyphs: Glyphs::default(),
        }
    }
}

/// A URL in a body, which the user opens by its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Where it leads.
    pub url: String,
    /// What it says in the body: its text, its image's description, or the URL itself.
    pub text: String,
}

/// Where an image's bytes are.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImageSource {
    /// A Part of the Message, by its `Content-ID` (without the angle brackets): an Inline Image.
    Cid(String),
    /// On the web: Remote Content.
    Remote(String),
    /// In the body itself, from a `data:` URL.
    Data(Vec<u8>),
}

/// An image a body shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// Where its bytes are.
    pub source: ImageSource,
    /// Its description, from `alt` or `title`; maybe empty.
    pub alt: String,
    /// The width the HTML gives it, if any.
    pub width: Option<Length>,
    /// The height the HTML gives it, if any.
    pub height: Option<Length>,
}

/// The pixel size of a terminal cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSize {
    /// Width, in pixels.
    pub width: u32,
    /// Height, in pixels.
    pub height: u32,
}

impl CellSize {
    /// How many pixels a CSS pixel likely takes on cells of this size: the terminal's font is about 20 pixels high
    /// on a screen of ordinary density, so cells twice as high are likely on a screen of twice the density.
    #[must_use]
    pub fn likely_pixel_ratio(self) -> f32 {
        // A cell's height is far below where f32 loses precision.
        #[allow(clippy::cast_precision_loss)]
        let ratio = (self.height as f32 / 20.0).round();
        ratio.clamp(1.0, 3.0)
    }
}

impl Default for CellSize {
    /// What's assumed where the terminal doesn't say.
    fn default() -> Self {
        Self {
            width: 10,
            height: 20,
        }
    }
}

impl Image {
    /// The cells, columns then rows, its Picture takes when its pixels are `pixels`, at most `max_cols` wide and
    /// `max_rows` high on cells of `cell` pixels, `ratio` of them to a CSS pixel: the size the HTML gives it, or else
    /// its own, in CSS pixels, shrunk to fit and never enlarged. `None` for an image too small to be worth lines of
    /// its own: a Tracking Pixel, a spacer, an icon.
    #[must_use]
    pub fn cells(
        &self,
        pixels: (u32, u32),
        max_cols: u16,
        max_rows: u16,
        cell: CellSize,
        ratio: f32,
    ) -> Option<(u16, u16)> {
        let (natural_width, natural_height) = (f64::from(pixels.0), f64::from(pixels.1));
        if natural_width <= 3.0 || natural_height <= 3.0 || max_cols == 0 || max_rows == 0 {
            return None;
        }
        // Cells measured in CSS pixels.
        let ratio = f64::from(ratio).max(0.25);
        let (cell_width, cell_height) = (
            f64::from(cell.width) / ratio,
            f64::from(cell.height) / ratio,
        );
        let container = f64::from(max_cols) * cell_width;
        let length = |length: Option<Length>| match length {
            Some(Length::Pixels(pixels)) => Some(f64::from(pixels)),
            Some(Length::Percent(percent)) => Some(container * f64::from(percent) / 100.0),
            None => None,
        };
        let aspect = natural_height / natural_width;
        let (width, height) = match (length(self.width), length(self.height)) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, width * aspect),
            (None, Some(height)) => (height / aspect, height),
            (None, None) => (natural_width, natural_height),
        };
        if width <= 3.0 || height <= 3.0 {
            return None;
        }
        let scale = 1f64
            .min(container / width)
            .min(f64::from(max_rows) * cell_height / height);
        let cells = |pixels: f64, per_cell: f64, max: u16| {
            // Clamped to a u16's range.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let cells = (pixels * scale / per_cell)
                .round()
                .clamp(1.0, f64::from(max)) as u16;
            cells
        };
        let (cols, rows) = (
            cells(width, cell_width, max_cols),
            cells(height, cell_height, max_rows),
        );
        // An icon, like a social network's in a footer, is better as its description in the text.
        (rows > 1 || cols > 4).then_some((cols, rows))
    }
}

/// How a plain text body is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextFormat {
    /// Every new line breaks the line.
    #[default]
    Fixed,
    /// RFC 3676's `format=flowed`: a line ending in a space goes on with the next; `delete_space` (`delsp=yes`) drops
    /// that space.
    Flowed {
        /// Whether the space ending a flowed line goes, as `delsp=yes` says.
        delete_space: bool,
    },
}

/// A body, read and ready to lay out.
#[derive(Clone, Debug)]
pub struct Document {
    blocks: Vec<Block>,
    links: Vec<Link>,
    images: Vec<Image>,
    styles: Styles,
}

impl Document {
    /// Reads an HTML body, drawn in `styles`.
    #[must_use]
    pub fn html(html: &str, styles: &Styles) -> Self {
        html::read(html, styles)
    }

    /// Reads a plain text body, drawn in `styles`.
    #[must_use]
    pub fn plain(text: &str, format: TextFormat, styles: &Styles) -> Self {
        plain::read(text, format, styles)
    }

    /// Its Links, in order: Link `n` is at index `n - 1`.
    #[must_use]
    pub fn links(&self) -> &[Link] {
        &self.links
    }

    /// Its images, in order, Tracking Pixels left out. [`Options::pictures`] says which can be drawn, by index here.
    #[must_use]
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// Lays it out as `options` say.
    #[must_use]
    pub fn render(&self, options: &Options<'_>) -> Rendering {
        layout::render(self, options)
    }
}

/// How to lay a [`Document`] out.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// The width of the lines, in cells.
    pub width: u16,
    /// The pixel size of the terminal's cells, to size Pictures.
    pub cell: CellSize,
    /// How many of the terminal's pixels make a CSS pixel: 2 on most HiDPI screens. HTML sizes images in CSS
    /// pixels, as browsers draw them.
    pub pixel_ratio: f32,
    /// The most rows a Picture takes.
    pub max_picture_rows: u16,
    /// The pixel size of each of the document's images that can be drawn, by its index in [`Document::images`];
    /// `None`, or a missing entry, shows its description instead.
    pub pictures: &'a [Option<(u32, u32)>],
}

/// A [`Document`] laid out.
#[derive(Clone, Debug, Default)]
pub struct Rendering {
    /// Its lines, at most as wide as asked. A Picture's rows are blank, for the caller to draw over.
    pub lines: Vec<Line<'static>>,
    /// Where each Picture goes.
    pub pictures: Vec<PictureSlot>,
}

/// Where a Picture goes in a [`Rendering`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureSlot {
    /// Its image's index in [`Document::images`].
    pub image: usize,
    /// The line its first row is drawn on.
    pub line: usize,
    /// The column it starts at.
    pub column: u16,
    /// Its width, in cells.
    pub cols: u16,
    /// Its height, in cells.
    pub rows: u16,
}
