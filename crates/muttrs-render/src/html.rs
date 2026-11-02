//! HTML read into blocks and flows, with the CSS a terminal can show applied.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ego_tree::{NodeId, NodeRef};
use ratatui_core::style::{Color, Modifier, Style};
use scraper::{ElementRef, Html, Node, Selector};

use crate::color::readable;
use crate::css::{self, Declaration, Length, Specificity};
use crate::ir::{Align, Block, Cell, Flow, Frame, Item, Row, Table, WhiteSpace};
use crate::{Document, Image, ImageSource, Link, Styles};

pub(crate) fn read(html: &str, styles: &Styles) -> Document {
    let dom = Html::parse_document(html);
    let mut reader = Reader {
        styles,
        sheet: Sheet::of(&dom),
        links: Vec::new(),
        link_numbers: HashMap::new(),
        images: Vec::new(),
    };
    let mut blocks = Vec::new();
    let context = Context {
        style: styles.text,
        align: Align::Left,
        white_space: WhiteSpace::Collapse,
        in_link: false,
        zero_font: false,
        uppercase: false,
        list_depth: 0,
    };
    reader.children(*dom.root_element(), &context, &mut blocks);
    Document {
        blocks,
        links: reader.links,
        images: reader.images,
        styles: styles.clone(),
    }
}

/// What an element passes on to what it holds, as CSS inherits.
#[derive(Clone, Debug)]
struct Context {
    style: Style,
    align: Align,
    white_space: WhiteSpace,
    in_link: bool,
    /// Under `font-size: 0`, which hides text, as preheaders do, until something sets a size again.
    zero_font: bool,
    uppercase: bool,
    list_depth: usize,
}

struct Reader<'a> {
    styles: &'a Styles,
    sheet: Sheet,
    links: Vec<Link>,
    /// Each URL's number, so that a URL linked twice, as an image and its caption often are, has one.
    link_numbers: HashMap<String, usize>,
    images: Vec<Image>,
}

impl Reader<'_> {
    fn children(&mut self, node: NodeRef<'_, Node>, context: &Context, out: &mut Vec<Block>) {
        for child in node.children() {
            match child.value() {
                Node::Text(text) => push_text(text, context, out),
                Node::Element(_) => {
                    let element = ElementRef::wrap(child).expect("an element");
                    self.element(element, context, out);
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, element: ElementRef<'_>, parent: &Context, out: &mut Vec<Block>) {
        let name = element.value().name();
        if SKIPPED.contains(&name) {
            return;
        }
        let props = self.sheet.props(element);
        if is_hidden(&props, element) {
            return;
        }
        let mut context = self.inherit(parent, &props, element);
        match name {
            "br" => flow(out, &context).items.push(Item::Break),
            "img" => self.image(element, &props, &context, out),
            "a" => self.link(element, &context, out),
            "hr" => out.extend([Block::Gap, Block::Rule, Block::Gap]),
            "p" | "dl" | "figure" => self.block(element, &context, Frame::Plain, true, out),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                context.style = context.style.patch(self.styles.heading);
                self.block(element, &context, Frame::Plain, true, out);
            }
            "blockquote" => self.block(element, &context, Frame::Quote, true, out),
            "dd" => self.block(element, &context, Frame::Indent(4), false, out),
            "ul" | "ol" | "menu" | "dir" => self.list(element, name == "ol", &props, &context, out),
            "li" => {
                let marker = format!("{} ", self.bullet(context.list_depth));
                self.block(element, &context, Frame::Item(marker), false, out);
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                context.white_space = WhiteSpace::Pre;
                context.style = context.style.patch(self.styles.code);
                self.block(element, &context, Frame::Plain, true, out);
            }
            "table" => self.table(element, &context, out),
            "q" => {
                flow(out, &context).push_text("“", context.style);
                self.children(*element, &context, out);
                flow(out, &context).push_text("”", context.style);
            }
            _ if BLOCKS.contains(&name) || is_block_display(&props) => {
                if props.get("display") == Some("inline") {
                    self.children(*element, &context, out);
                } else {
                    self.block(element, &context, Frame::Plain, false, out);
                }
            }
            _ => self.children(*element, &context, out),
        }
    }

    /// The context of `element`'s content, from `parent`'s, its styles and its tag.
    fn inherit(&self, parent: &Context, props: &Props, element: ElementRef<'_>) -> Context {
        let mut context = parent.clone();
        let mut style = parent.style;
        let name = element.value().name();
        match name {
            "b" | "strong" | "th" => style = style.add_modifier(Modifier::BOLD),
            "i" | "em" | "cite" | "dfn" | "var" => style = style.add_modifier(Modifier::ITALIC),
            "u" | "ins" => style = style.add_modifier(Modifier::UNDERLINED),
            "s" | "strike" | "del" => style = style.add_modifier(Modifier::CROSSED_OUT),
            "code" | "kbd" | "samp" | "tt" => style = style.patch(self.styles.code),
            "mark" => style = style.patch(self.styles.mark),
            "center" => context.align = Align::Center,
            _ => {}
        }
        let classes: Vec<&str> = element.value().classes().collect();
        if classes
            .iter()
            .any(|class| matches!(*class, "gmail_attr" | "moz-cite-prefix"))
        {
            style = style.patch(self.styles.attribution);
        }
        if classes
            .iter()
            .any(|class| matches!(*class, "gmail_signature" | "moz-signature"))
        {
            style = style.patch(self.styles.signature);
        }

        let color = props
            .get("color")
            .or_else(|| (name == "font").then(|| element.attr("color")).flatten());
        if let (Some(color), Some(page)) = (color.and_then(css::color), self.styles.page) {
            style.fg = match readable(color, page) {
                Some(readable) => Some(Color::Rgb(readable.0, readable.1, readable.2)),
                None => self.styles.text.fg,
            };
        }
        match props.get("font-weight") {
            Some("bold" | "bolder" | "600" | "700" | "800" | "900") => {
                style = style.add_modifier(Modifier::BOLD);
            }
            Some("normal" | "lighter" | "100" | "200" | "300" | "400" | "500") => {
                style = style.remove_modifier(Modifier::BOLD);
            }
            _ => {}
        }
        match props.get("font-style") {
            Some("italic" | "oblique") => style = style.add_modifier(Modifier::ITALIC),
            Some("normal") => style = style.remove_modifier(Modifier::ITALIC),
            _ => {}
        }
        if let Some(decoration) = props
            .get("text-decoration-line")
            .or_else(|| props.get("text-decoration"))
        {
            if decoration.contains("none") {
                style = style.remove_modifier(Modifier::UNDERLINED | Modifier::CROSSED_OUT);
            }
            if decoration.contains("underline") {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            if decoration.contains("line-through") {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
        }
        context.style = style;

        let align = props.get("text-align").or_else(|| {
            (!matches!(name, "table" | "img"))
                .then(|| element.attr("align"))
                .flatten()
        });
        match align.map(str::to_ascii_lowercase).as_deref() {
            Some("center" | "middle") => context.align = Align::Center,
            Some("right" | "end") => context.align = Align::Right,
            Some("left" | "start" | "justify") => context.align = Align::Left,
            _ => {}
        }
        match props.get("white-space") {
            Some("pre") => context.white_space = WhiteSpace::Pre,
            Some("pre-wrap" | "pre-line" | "break-spaces") => {
                context.white_space = WhiteSpace::PreWrap;
            }
            Some("normal" | "nowrap") => context.white_space = WhiteSpace::Collapse,
            _ => {}
        }
        if let Some(size) = props.get("font-size") {
            context.zero_font = css::is_zero_size(size);
        }
        match props.get("text-transform") {
            Some("uppercase") => context.uppercase = true,
            Some("none") => context.uppercase = false,
            _ => {}
        }
        context
    }

    /// `element`'s content as blocks of their own, in `frame`, between gaps if `spaced`.
    fn block(
        &mut self,
        element: ElementRef<'_>,
        context: &Context,
        frame: Frame,
        spaced: bool,
        out: &mut Vec<Block>,
    ) {
        let mut blocks = Vec::new();
        self.children(*element, context, &mut blocks);
        if spaced {
            out.push(Block::Gap);
        }
        out.push(Block::Stack(blocks, frame));
        if spaced {
            out.push(Block::Gap);
        }
    }

    fn bullet(&self, depth: usize) -> &str {
        let bullets = &self.styles.glyphs.bullets;
        if bullets.is_empty() {
            "-"
        } else {
            &bullets[depth.saturating_sub(1) % bullets.len()]
        }
    }

    fn list(
        &mut self,
        element: ElementRef<'_>,
        ordered: bool,
        props: &Props,
        context: &Context,
        out: &mut Vec<Block>,
    ) {
        let mut inner = context.clone();
        inner.list_depth += 1;
        // A centred item's text would stand away from its marker.
        inner.align = Align::Left;
        let kind = props
            .get("list-style-type")
            .or_else(|| props.get("list-style"))
            .map(|kind| kind.split_whitespace().next().unwrap_or(kind))
            .or_else(|| element.attr("type"))
            .unwrap_or(if ordered { "1" } else { "disc" })
            .to_owned();
        let mut number: i64 = element
            .attr("start")
            .and_then(|start| start.trim().parse().ok())
            .unwrap_or(1);
        let mut items: Vec<(String, Vec<Block>)> = Vec::new();
        for child in element.children() {
            let Some(item) = ElementRef::wrap(child) else {
                // Text between items: whatever isn't white space goes in an item of its own.
                if let Node::Text(text) = child.value()
                    && !text.trim().is_empty()
                {
                    let mut blocks = Vec::new();
                    push_text(text, &inner, &mut blocks);
                    items.push((String::new(), blocks));
                }
                continue;
            };
            let props = self.sheet.props(item);
            if is_hidden(&props, item) {
                continue;
            }
            let mut blocks = Vec::new();
            if item.value().name() != "li" {
                // A list right inside a list, as mail often nests them: indented under the last item.
                self.element(item, &inner, &mut blocks);
                items.push((String::new(), vec![Block::Stack(blocks, Frame::Indent(2))]));
                continue;
            }
            if let Some(value) = item.attr("value").and_then(|v| v.trim().parse().ok()) {
                number = value;
            }
            let item_context = self.inherit(&inner, &props, item);
            self.children(*item, &item_context, &mut blocks);
            let marker = if ordered {
                numeral(number, &kind).map(|n| format!("{n}. "))
            } else if kind == "none" {
                None
            } else {
                Some(format!("{} ", self.bullet(inner.list_depth)))
            };
            items.push((marker.unwrap_or_default(), blocks));
            number += 1;
        }
        // Numbers line up on their right, so that items line up on their left.
        let marker_width = items
            .iter()
            .map(|(marker, _)| marker.chars().count())
            .max()
            .unwrap_or(0);
        if context.list_depth == 0 {
            out.push(Block::Gap);
        }
        for (marker, blocks) in items {
            if marker.is_empty() {
                out.push(Block::Stack(
                    blocks,
                    Frame::Indent(u16::try_from(marker_width).unwrap_or(0)),
                ));
            } else {
                let padded = format!("{marker:>marker_width$}");
                out.push(Block::Stack(blocks, Frame::Item(padded)));
            }
        }
        if context.list_depth == 0 {
            out.push(Block::Gap);
        }
    }

    fn link(&mut self, element: ElementRef<'_>, context: &Context, out: &mut Vec<Block>) {
        let url = element
            .attr("href")
            .map(str::trim)
            .filter(|href| is_followable(href));
        let has_content = !element.text().collect::<String>().trim().is_empty()
            || element
                .descendent_elements()
                .any(|e| e.value().name() == "img");
        let (Some(url), true) = (url, has_content) else {
            self.children(*element, context, out);
            return;
        };
        let number = if let Some(&number) = self.link_numbers.get(url) {
            number
        } else {
            self.links.push(Link {
                url: url.to_owned(),
                text: String::new(),
            });
            self.link_numbers.insert(url.to_owned(), self.links.len());
            self.links.len()
        };
        let link = &mut self.links[number - 1];
        if link.text.is_empty() {
            let text = collapse(&clean(&element.text().collect::<String>()));
            link.text = match text.trim() {
                "" => element
                    .descendent_elements()
                    .find_map(|e| e.attr("alt").filter(|alt| !alt.trim().is_empty()))
                    .map_or_else(|| url.to_owned(), |alt| alt.trim().to_owned()),
                text => text.to_owned(),
            };
        }
        let mut inner = context.clone();
        inner.in_link = true;
        inner.style = context.style.patch(self.styles.link);
        self.children(*element, &inner, out);
        flow(out, context).push_text(&format!("[{number}]"), self.styles.link_number);
    }

    fn image(
        &mut self,
        element: ElementRef<'_>,
        props: &Props,
        context: &Context,
        out: &mut Vec<Block>,
    ) {
        let Some(source) = element.attr("src").and_then(image_source) else {
            return;
        };
        let length = |name: &str| {
            props
                .get(name)
                .and_then(Length::parse)
                .or_else(|| element.attr(name).and_then(Length::parse))
        };
        let (width, height) = (length("width"), length("height"));
        let tiny = |length: Option<Length>| {
            matches!(length, Some(Length::Pixels(pixels)) if pixels <= 3.0)
                || length.is_some_and(Length::is_zero)
        };
        if tiny(width) || tiny(height) {
            // A Tracking Pixel, or a spacer.
            return;
        }
        let alt = element
            .attr("alt")
            .or_else(|| element.attr("title"))
            .map(|alt| collapse(&clean(alt)).trim().to_owned())
            .unwrap_or_default();
        self.images.push(Image {
            source,
            alt,
            width,
            height,
        });
        let style = if context.in_link {
            context.style
        } else {
            context.style.patch(self.styles.image)
        };
        flow(out, context)
            .items
            .push(Item::Image(self.images.len() - 1, style));
    }

    fn table(&mut self, table: ElementRef<'_>, context: &Context, out: &mut Vec<Block>) {
        let mut rows: Vec<(ElementRef<'_>, bool)> = Vec::new();
        let mut caption = None;
        for child in table.child_elements() {
            match child.value().name() {
                "tr" => rows.push((child, false)),
                "thead" | "tbody" | "tfoot" => rows.extend(
                    child
                        .child_elements()
                        .filter(|row| row.value().name() == "tr")
                        .map(|row| (row, child.value().name() == "thead")),
                ),
                "caption" => caption = Some(child),
                _ => {}
            }
        }
        if is_layout(table, &rows) {
            let mut blocks = Vec::new();
            if let Some(caption) = caption {
                self.element(caption, context, &mut blocks);
            }
            for (row, _) in &rows {
                let props = self.sheet.props(*row);
                if is_hidden(&props, *row) {
                    continue;
                }
                let row_context = self.inherit(context, &props, *row);
                for cell in cells(*row) {
                    self.element(cell, &row_context, &mut blocks);
                }
            }
            out.push(Block::Stack(blocks, Frame::Plain));
            return;
        }

        let mut grid = Table::default();
        for (row, in_head) in rows {
            let props = self.sheet.props(row);
            if is_hidden(&props, row) {
                continue;
            }
            let row_context = self.inherit(context, &props, row);
            let row_cells = cells(row);
            let header = in_head || row_cells.iter().all(|cell| cell.value().name() == "th");
            let mut drawn = Vec::new();
            for cell in row_cells {
                let props = self.sheet.props(cell);
                if is_hidden(&props, cell) {
                    continue;
                }
                let mut cell_context = self.inherit(&row_context, &props, cell);
                if header || cell.value().name() == "th" {
                    cell_context.style = cell_context.style.patch(self.styles.table_header);
                }
                let mut blocks = Vec::new();
                self.children(*cell, &cell_context, &mut blocks);
                drawn.push(Cell {
                    blocks,
                    span: cell
                        .attr("colspan")
                        .and_then(|span| span.trim().parse().ok())
                        .unwrap_or(1usize)
                        .clamp(1, 64),
                });
            }
            grid.rows.push(Row {
                cells: drawn,
                header,
            });
        }
        out.push(Block::Gap);
        if let Some(caption) = caption {
            self.element(caption, context, out);
        }
        out.push(Block::Table(grid));
        out.push(Block::Gap);
    }
}

/// Adds the text of a text node to `out`, as `context` shows it.
fn push_text(text: &str, context: &Context, out: &mut Vec<Block>) {
    if context.zero_font {
        return;
    }
    let mut text = clean(text);
    if context.white_space == WhiteSpace::Collapse {
        text = collapse(&text);
        // White space between blocks is no text at all.
        if text == " " && !matches!(out.last(), Some(Block::Flow(_))) {
            return;
        }
    }
    if context.uppercase {
        text = text.to_uppercase();
    }
    flow(out, context).push_text(&text, context.style);
}

/// Whether `table` lays a page out rather than holding data, so that it's drawn as its cells one after the other.
fn is_layout(table: ElementRef<'_>, rows: &[(ElementRef<'_>, bool)]) -> bool {
    if matches!(table.attr("role"), Some("presentation" | "none")) {
        return true;
    }
    let columns = rows
        .iter()
        .map(|(row, _)| {
            cells(*row)
                .iter()
                .map(|cell| {
                    cell.attr("colspan")
                        .and_then(|span| span.trim().parse::<usize>().ok())
                        .unwrap_or(1)
                })
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    if columns < 2 || rows.len() < 2 {
        return true;
    }
    let inside = |names: &[&str]| {
        table
            .descendent_elements()
            .skip(1)
            .any(|e| names.contains(&e.value().name()))
    };
    if inside(&["table", "img"]) {
        return true;
    }
    let bordered = table
        .attr("border")
        .and_then(|border| border.trim().parse::<u32>().ok())
        .is_some_and(|border| border > 0);
    if bordered || inside(&["th"]) {
        return false;
    }
    // Short cells of text alone: a table of data, like a receipt's.
    let short = rows.iter().all(|(row, _)| {
        cells(*row).iter().all(|cell| {
            cell.text()
                .map(|text| text.trim().chars().count())
                .sum::<usize>()
                <= 50
        })
    });
    !short
        || inside(&[
            "p",
            "div",
            "h1",
            "h2",
            "h3",
            "ul",
            "ol",
            "blockquote",
            "pre",
        ])
}

/// A row's cells.
fn cells(row: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    row.child_elements()
        .filter(|cell| matches!(cell.value().name(), "td" | "th"))
        .collect()
}

/// The flow inline content goes in: the last block of `out` if it's a flow laid out as `context` says, or a new one.
fn flow<'o>(out: &'o mut Vec<Block>, context: &Context) -> &'o mut Flow {
    let fits = matches!(out.last(), Some(Block::Flow(flow))
        if flow.align == context.align && flow.white_space == context.white_space);
    if !fits {
        out.push(Block::Flow(Flow::new(context.align, context.white_space)));
    }
    match out.last_mut() {
        Some(Block::Flow(flow)) => flow,
        _ => unreachable!("a flow was just pushed"),
    }
}

/// Elements never drawn: metadata, scripts, and what a terminal can't show.
const SKIPPED: &[&str] = &[
    "head", "style", "script", "title", "meta", "link", "base", "template", "noscript", "svg",
    "math", "object", "embed", "iframe", "frame", "frameset", "input", "select", "textarea",
    "button", "map", "area", "audio", "video", "canvas", "xml",
];

/// Elements that are blocks by default, besides those with a role of their own.
const BLOCKS: &[&str] = &[
    "html",
    "body",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "figcaption",
    "address",
    "fieldset",
    "legend",
    "form",
    "details",
    "summary",
    "center",
    "hgroup",
    "dt",
    "tr",
    "td",
    "th",
    "thead",
    "tbody",
    "tfoot",
    "caption",
    "option",
];

fn is_block_display(props: &Props) -> bool {
    props.get("display").is_some_and(|display| {
        matches!(
            display,
            "block"
                | "flex"
                | "grid"
                | "list-item"
                | "table"
                | "table-row"
                | "table-cell"
                | "flow-root"
        )
    })
}

/// Whether `element`, with `props`, is hidden: as preheaders, and what's for another client than this one, are.
fn is_hidden(props: &Props, element: ElementRef<'_>) -> bool {
    let zero = |name: &str| {
        props
            .get(name)
            .and_then(Length::parse)
            .is_some_and(Length::is_zero)
    };
    element.attr("hidden").is_some()
        || props.get("display") == Some("none")
        || matches!(props.get("visibility"), Some("hidden" | "collapse"))
        || props.get("mso-hide") == Some("all")
        || props.get("opacity").and_then(|o| o.parse::<f32>().ok()) == Some(0.0)
        || (props.get("overflow") == Some("hidden")
            && (zero("max-height") || zero("height") || zero("max-width") || zero("width")))
}

fn is_followable(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    ["http://", "https://", "mailto:", "ftp://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

fn image_source(src: &str) -> Option<ImageSource> {
    let src = src.trim();
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("cid:") {
        let id = src[4..]
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        return Some(ImageSource::Cid(id.to_owned()));
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(ImageSource::Remote(src.to_owned()));
    }
    if lower.starts_with("//") {
        return Some(ImageSource::Remote(format!("https:{src}")));
    }
    if lower.starts_with("data:") {
        let (header, payload) = src[5..].split_once(',')?;
        if !header.to_ascii_lowercase().ends_with(";base64") {
            return None;
        }
        let payload: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
        return BASE64.decode(payload).ok().map(ImageSource::Data);
    }
    None
}

/// A list item's number as `kind` (HTML's `type`, or CSS's `list-style-type`) writes it; `None` for `none`.
fn numeral(number: i64, kind: &str) -> Option<String> {
    let alphabetic = |upper: bool| {
        let mut n = number;
        if n < 1 {
            return number.to_string();
        }
        let mut letters = Vec::new();
        while n > 0 {
            n -= 1;
            // Below 26 by the modulo.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            letters.push(char::from(b'a' + (n % 26) as u8));
            n /= 26;
        }
        let word: String = letters.into_iter().rev().collect();
        if upper { word.to_uppercase() } else { word }
    };
    let roman = |upper: bool| {
        if !(1..4000).contains(&number) {
            return number.to_string();
        }
        let mut n = number;
        let mut word = String::new();
        for (value, letters) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while n >= value {
                word.push_str(letters);
                n -= value;
            }
        }
        if upper { word.to_uppercase() } else { word }
    };
    match kind {
        "none" => None,
        "a" | "lower-alpha" | "lower-latin" => Some(alphabetic(false)),
        "A" | "upper-alpha" | "upper-latin" => Some(alphabetic(true)),
        "i" | "lower-roman" => Some(roman(false)),
        "I" | "upper-roman" => Some(roman(true)),
        _ => Some(number.to_string()),
    }
}

/// `text` without the invisible characters newsletters pad their preheaders with, its other spaces plain spaces.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\u{034F}' | '\u{200B}' | '\u{FEFF}' | '\u{00AD}' | '\u{2060}' | '\u{180E}' => {}
            // A zero-width non-joiner joins nothing next to space: there, it's padding.
            '\u{200C}' => {
                let before = out.chars().next_back().is_none_or(char::is_whitespace);
                let after = chars.peek().is_none_or(|next| {
                    next.is_whitespace() || *next == '\u{200C}' || *next == '\u{034F}'
                });
                if !(before || after) {
                    out.push(character);
                }
            }
            '\u{00A0}' | '\u{2007}' | '\u{202F}' => out.push(' '),
            _ => out.push(character),
        }
    }
    out
}

/// `text` with every run of white space one space, as HTML shows it.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(character);
            in_space = false;
        }
    }
    out
}

/// An element's computed declarations: from the style sheets, then its `style` attribute, by precedence.
#[derive(Debug, Default)]
struct Props(HashMap<String, String>);

impl Props {
    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

/// Which declarations of the document's style sheets apply to which elements.
struct Sheet {
    matched: HashMap<NodeId, Vec<(Rank, Declaration)>>,
}

/// A declaration's precedence: importance, then specificity (an inline style's being the highest), then order.
type Rank = (bool, Specificity, usize);

impl Sheet {
    fn of(dom: &Html) -> Self {
        let mut matched: HashMap<NodeId, Vec<(Rank, Declaration)>> = HashMap::new();
        let style = Selector::parse("style").expect("a valid selector");
        let mut order = 0;
        for element in dom.select(&style) {
            let text: String = element.text().collect();
            for rule in css::stylesheet(&text) {
                order += 1;
                // Selectors scraper can't parse, such as pseudo-elements, match nothing.
                let Ok(selector) = Selector::parse(&rule.selector) else {
                    continue;
                };
                for element in dom.select(&selector) {
                    let declarations = matched.entry(element.id()).or_default();
                    for declaration in &rule.declarations {
                        declarations.push((
                            (declaration.important, rule.specificity, order),
                            declaration.clone(),
                        ));
                    }
                }
            }
        }
        Self { matched }
    }

    fn props(&self, element: ElementRef<'_>) -> Props {
        let mut declarations = self.matched.get(&element.id()).cloned().unwrap_or_default();
        if let Some(style) = element.attr("style") {
            declarations.extend(
                css::declarations(style)
                    .into_iter()
                    .map(|declaration| ((declaration.important, (u16::MAX, 0, 0), 0), declaration)),
            );
        }
        declarations.sort_by_key(|(rank, _)| *rank);
        Props(
            declarations
                .into_iter()
                .map(|(_, declaration)| (declaration.name, declaration.value.to_ascii_lowercase()))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spacer_characters_go_and_spaces_collapse() {
        assert_eq!(
            collapse(&clean(
                "Hi\u{00A0}\u{200C}\u{034F} \u{200C}\u{00A0}there\u{00AD}!"
            )),
            "Hi there!"
        );
        assert_eq!(
            clean("می\u{200C}خواهم"),
            "می\u{200C}خواهم",
            "a joiner between letters stays"
        );
    }

    #[test]
    fn numerals() {
        assert_eq!(numeral(3, "1").as_deref(), Some("3"));
        assert_eq!(numeral(28, "a").as_deref(), Some("ab"));
        assert_eq!(numeral(14, "I").as_deref(), Some("XIV"));
        assert_eq!(numeral(1, "none"), None);
    }

    #[test]
    fn image_sources() {
        assert_eq!(
            image_source("cid:<logo@x>"),
            Some(ImageSource::Cid("logo@x".into()))
        );
        assert_eq!(
            image_source("//cdn.example.com/a.png"),
            Some(ImageSource::Remote("https://cdn.example.com/a.png".into()))
        );
        assert_eq!(
            image_source("data:image/gif;base64,R0lG"),
            Some(ImageSource::Data(b"GIF".to_vec()))
        );
        assert_eq!(image_source("file:///etc/passwd"), None);
    }
}
