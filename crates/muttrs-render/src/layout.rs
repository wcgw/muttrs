//! Blocks laid out for a width: stacked, framed, and their flows wrapped into lines.

use ratatui_core::style::Style;
use ratatui_core::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::ir::{Align, Block, Flow, Frame, Item, WhiteSpace};
use crate::{Document, Options, PictureSlot, Rendering, table};

pub(crate) fn render(document: &Document, options: &Options<'_>) -> Rendering {
    let cx = Cx {
        document,
        options,
        quote_depth: 0,
        pictures: true,
        measuring: false,
    };
    let mut fragment = cx.blocks(&document.blocks, options.width.max(1));
    fragment.trim_blank_ends();
    Rendering {
        lines: fragment.lines,
        pictures: fragment.pictures,
    }
}

/// What laying out needs to know as it goes down the blocks.
#[derive(Clone, Copy)]
pub(crate) struct Cx<'a> {
    pub(crate) document: &'a Document,
    pub(crate) options: &'a Options<'a>,
    /// How many quotes deep the blocks being laid out are.
    quote_depth: usize,
    /// Whether images may be drawn as Pictures here; not in a table's cells.
    pub(crate) pictures: bool,
    /// Whether the lines are only measured, so aren't aligned.
    measuring: bool,
}

/// Lines laid out, and the gaps asked for before and after them, which the blocks around settle.
#[derive(Debug, Default)]
pub(crate) struct Fragment {
    pub(crate) lines: Vec<Line<'static>>,
    /// Whether each line is blank, as a gap is; a Picture's rows aren't.
    blank: Vec<bool>,
    pub(crate) pictures: Vec<PictureSlot>,
    gap_before: bool,
    gap_after: bool,
}

impl Fragment {
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Asks for a blank line between what's laid out so far and what comes next.
    pub(crate) fn push_gap(&mut self) {
        self.gap();
    }

    fn gap(&mut self) {
        if self.lines.is_empty() {
            self.gap_before = true;
        } else {
            self.gap_after = true;
        }
    }

    /// Settles a gap asked for, before more lines go in.
    fn settle(&mut self) {
        if self.gap_after {
            self.gap_after = false;
            if self.blank.last() == Some(&false) {
                self.lines.push(Line::default());
                self.blank.push(true);
            }
        }
    }

    pub(crate) fn push(&mut self, line: Line<'static>) {
        self.settle();
        let blank = line.spans.iter().all(|span| span.content.trim().is_empty());
        self.lines.push(line);
        self.blank.push(blank);
    }

    /// Puts `other` after what's here, settling the gaps between them.
    pub(crate) fn append(&mut self, other: Fragment) {
        if other.is_empty() {
            if other.gap_before || other.gap_after {
                self.gap();
            }
            return;
        }
        if other.gap_before {
            self.gap();
        }
        self.settle();
        let offset = self.lines.len();
        self.lines.extend(other.lines);
        self.blank.extend(other.blank);
        self.pictures
            .extend(other.pictures.into_iter().map(|picture| PictureSlot {
                line: picture.line + offset,
                ..picture
            }));
        self.gap_after = other.gap_after;
    }

    /// Each line after `first` on the first line and `rest` on the others, which are `width` cells wide.
    fn framed(mut self, first: &[Span<'static>], rest: &[Span<'static>], width: u16) -> Self {
        for (index, line) in self.lines.iter_mut().enumerate() {
            let prefix = if index == 0 { first } else { rest };
            let mut spans = prefix.to_vec();
            spans.append(&mut line.spans);
            line.spans = spans;
        }
        for picture in &mut self.pictures {
            picture.column += width;
        }
        self
    }

    fn trim_blank_ends(&mut self) {
        let leading = self.blank.iter().take_while(|&&blank| blank).count();
        let trailing = self.blank[leading..]
            .iter()
            .rev()
            .take_while(|&&blank| blank)
            .count();
        self.lines.truncate(self.lines.len() - trailing);
        self.lines.drain(..leading);
        self.blank.truncate(self.blank.len() - trailing);
        self.blank.drain(..leading);
        for picture in &mut self.pictures {
            picture.line -= leading;
        }
    }
}

/// The narrowest a frame leaves for what it holds; below that, the frame is dropped rather than its content
/// squeezed.
const NARROWEST: u16 = 8;

impl Cx<'_> {
    pub(crate) fn without_pictures(self) -> Self {
        Self {
            pictures: false,
            ..self
        }
    }

    /// For laying out to measure how wide lines would be, unaligned.
    pub(crate) fn measuring(self) -> Self {
        Self {
            measuring: true,
            ..self
        }
    }

    pub(crate) fn blocks(&self, blocks: &[Block], width: u16) -> Fragment {
        let mut fragment = Fragment::default();
        for block in blocks {
            match block {
                Block::Gap => fragment.gap(),
                Block::Flow(flow) => fragment.append(self.flow(flow, width)),
                Block::Stack(blocks, frame) => fragment.append(self.stack(blocks, frame, width)),
                Block::Rule => {
                    let rule = "─".repeat(usize::from(width));
                    fragment.push(Line::from(Span::styled(rule, self.document.styles.rule)));
                }
                Block::Table(grid) => fragment.append(table::layout(self, grid, width)),
            }
        }
        fragment
    }

    fn stack(&self, blocks: &[Block], frame: &Frame, width: u16) -> Fragment {
        let styles = &self.document.styles;
        let (first, rest, frame_width, inner) = match frame {
            Frame::Plain => return self.blocks(blocks, width),
            Frame::Quote => {
                let style = if styles.quotes.is_empty() {
                    Style::new()
                } else {
                    styles.quotes[self.quote_depth % styles.quotes.len()]
                };
                let bar = vec![
                    Span::styled(styles.glyphs.quote.clone(), style),
                    Span::raw(" "),
                ];
                let inner = Self {
                    quote_depth: self.quote_depth + 1,
                    ..*self
                };
                (bar.clone(), bar, 2, inner)
            }
            Frame::Item(marker) => {
                // Narrower than u16::MAX by NARROWEST's check below, if not here.
                let marker_width = u16::try_from(marker.width()).unwrap_or(u16::MAX);
                let first = vec![Span::styled(marker.clone(), styles.text)];
                let rest = vec![Span::raw(" ".repeat(usize::from(marker_width)))];
                (first, rest, marker_width, *self)
            }
            Frame::Indent(cells) => {
                let indent = vec![Span::raw(" ".repeat(usize::from(*cells)))];
                (indent.clone(), indent, *cells, *self)
            }
        };
        if width < frame_width.saturating_add(NARROWEST) {
            return inner.blocks(blocks, width);
        }
        let mut fragment = inner.blocks(blocks, width - frame_width);
        // An empty item still shows its marker.
        if fragment.is_empty() && matches!(frame, Frame::Item(_)) {
            fragment.push(Line::default());
        }
        let (gap_before, gap_after) = (fragment.gap_before, fragment.gap_after);
        let mut framed = fragment.framed(&first, &rest, frame_width);
        // The gaps at the ends of a quote go outside its bar.
        framed.gap_before = gap_before;
        framed.gap_after = gap_after;
        framed
    }

    pub(crate) fn flow(&self, flow: &Flow, width: u16) -> Fragment {
        let align = if self.measuring {
            Align::Left
        } else {
            flow.align
        };
        let (fragment, wrapped) = self.wrap(flow, width, align);
        // Centred or right-aligned text that wraps is a paragraph, which reads better on the left: what's left
        // aligned so is headings, buttons and footers' lines, as newsletters mean them.
        if wrapped && align != Align::Left {
            return self.wrap(flow, width, Align::Left).0;
        }
        fragment
    }

    /// `flow`'s lines at `width`, aligned as `align` says, and whether any wrapped.
    fn wrap(&self, flow: &Flow, width: u16, align: Align) -> (Fragment, bool) {
        let mut lines = Lines {
            fragment: Fragment::default(),
            line: Vec::new(),
            line_width: 0,
            space: None,
            width: usize::from(width),
            align,
            white_space: flow.white_space,
            word_at: (0, 0, 0),
            wrapped: false,
        };
        for item in &flow.items {
            match item {
                Item::Text(text, style) => lines.text(text, *style),
                Item::Break => lines.hard_break(),
                Item::Image(index, style) => {
                    if let Some((cols, rows)) = self.picture(*index, width) {
                        lines.picture(*index, cols, rows);
                    } else {
                        let image = &self.document.images[*index];
                        if !image.alt.is_empty() {
                            let glyph = &self.document.styles.glyphs.image;
                            lines.text(&format!("[{glyph}{}]", image.alt), *style);
                        }
                    }
                }
            }
        }
        let wrapped = lines.wrapped;
        (lines.finish(), wrapped)
    }

    /// The cells image `index` takes as a Picture `width` cells wide at most, if it can be drawn here.
    fn picture(&self, index: usize, width: u16) -> Option<(u16, u16)> {
        if !self.pictures {
            return None;
        }
        let pixels = (*self.options.pictures.get(index)?)?;
        self.document.images[index].cells(
            pixels,
            width,
            self.options.max_picture_rows,
            self.options.cell,
            self.options.pixel_ratio,
        )
    }
}

/// The lines of a flow, as they fill.
struct Lines {
    fragment: Fragment,
    /// The line filling, and how wide it is so far.
    line: Vec<Span<'static>>,
    line_width: usize,
    /// White space waiting to go before the next word, if it fits on this line: its style, and how many cells.
    space: Option<(Style, usize)>,
    width: usize,
    align: Align,
    white_space: WhiteSpace,
    /// Where the word the line ends with starts: its span, the byte in that span, and the line's width before it.
    word_at: (usize, usize, usize),
    /// Whether a line ended for want of room.
    wrapped: bool,
}

impl Lines {
    fn text(&mut self, text: &str, style: Style) {
        let mut first = true;
        for line in text.split('\n') {
            if !first {
                self.hard_break();
            }
            first = false;
            let mut rest = line;
            while !rest.is_empty() {
                let space_end = rest
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(rest.len());
                if space_end > 0 {
                    let cells: usize = rest[..space_end]
                        .chars()
                        .map(|c| if c == '\t' { 4 } else { 1 })
                        .sum();
                    self.space(style, cells);
                    rest = &rest[space_end..];
                    continue;
                }
                let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                self.word(&rest[..word_end], style);
                rest = &rest[word_end..];
            }
        }
    }

    fn space(&mut self, style: Style, cells: usize) {
        match self.white_space {
            // Space at the start of a line is no space at all.
            WhiteSpace::Collapse if self.line.is_empty() => {}
            WhiteSpace::Collapse => self.space = Some((style, 1)),
            WhiteSpace::PreWrap | WhiteSpace::Pre => {
                let waiting = self.space.map_or(0, |(_, cells)| cells);
                self.space = Some((style, waiting + cells));
            }
        }
    }

    /// Adds a piece of a word: text without white space, which goes on this line only if it all fits, or with the
    /// rest of its word when it continues one.
    fn word(&mut self, word: &str, style: Style) {
        if self.white_space == WhiteSpace::Pre {
            self.flush_space();
            self.cut(word, style);
            return;
        }
        let width = word.width();
        let space = self.space.map_or(0, |(_, cells)| cells);
        let continues = self.space.is_none() && !self.line.is_empty();
        if self.line_width + space + width <= self.width {
            self.flush_space();
            if !continues {
                self.mark_word_start(style);
            }
            self.push(word, style);
            return;
        }
        if continues {
            // The rest of a word whose start is on this line, as in `<b>word</b>,`: the whole word moves down,
            // if it fits on a line.
            let (index, offset, at_width) = self.word_at;
            if at_width > 0 && self.line_width - at_width + width <= self.width {
                let mut moved = self.line.split_off(index);
                if offset > 0 {
                    let head = moved[0].content[..offset].to_owned();
                    let tail = moved[0].content[offset..].to_owned();
                    self.line.push(Span::styled(head, moved[0].style));
                    moved[0].content = tail.into();
                }
                // The space before the word ends the line.
                if let Some(last) = self.line.last_mut() {
                    let trimmed = last.content.trim_end().len();
                    last.content.to_mut().truncate(trimmed);
                }
                self.line_width = self.line.iter().map(|span| span.content.width()).sum();
                self.wrap_line();
                for span in moved {
                    self.push(&span.content, span.style);
                }
                self.push(word, style);
                return;
            }
            self.cut(word, style);
            return;
        }
        if !self.line.is_empty() {
            self.space = None;
            self.wrap_line();
        } else if self.white_space == WhiteSpace::PreWrap {
            // Indentation that doesn't fit with its word goes.
            self.space = None;
        }
        self.mark_word_start(style);
        self.cut(word, style);
    }

    /// Notes that a word starts here, where a piece in `style` is about to go.
    fn mark_word_start(&mut self, style: Style) {
        self.word_at = match self.line.last() {
            Some(last) if last.style == style => {
                (self.line.len() - 1, last.content.len(), self.line_width)
            }
            _ => (self.line.len(), 0, self.line_width),
        };
    }

    /// Adds `word` across as many lines as it takes, cut between graphemes.
    fn cut(&mut self, word: &str, style: Style) {
        let mut piece = String::new();
        let mut piece_width = 0;
        for grapheme in word.graphemes(true) {
            let width = grapheme.width();
            if self.line_width + piece_width + width > self.width
                && self.line_width + piece_width > 0
            {
                self.push(&piece, style);
                piece.clear();
                piece_width = 0;
                self.wrap_line();
            }
            piece.push_str(grapheme);
            piece_width += width;
        }
        self.push(&piece, style);
    }

    fn flush_space(&mut self) {
        if let Some((style, cells)) = self.space.take() {
            let cells = cells.min(self.width.saturating_sub(self.line_width));
            if cells > 0 {
                self.push(&" ".repeat(cells), style);
            }
        }
    }

    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.line_width += text.width();
        match self.line.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(text),
            _ => self.line.push(Span::styled(text.to_owned(), style)),
        }
    }

    /// Ends the line for want of room.
    fn wrap_line(&mut self) {
        self.wrapped = true;
        self.end_line();
    }

    fn end_line(&mut self) {
        let spans = std::mem::take(&mut self.line);
        let padding = if spans.is_empty() {
            0
        } else {
            self.padding(self.line_width)
        };
        self.line_width = 0;
        self.word_at = (0, 0, 0);
        let mut line = Line::from(spans);
        if padding > 0 {
            line.spans.insert(0, Span::raw(" ".repeat(padding)));
        }
        self.fragment.push(line);
    }

    /// The cells before a line `width` wide, as the flow is aligned.
    fn padding(&self, width: usize) -> usize {
        let free = self.width.saturating_sub(width);
        match self.align {
            Align::Left => 0,
            Align::Center => free / 2,
            Align::Right => free,
        }
    }

    fn hard_break(&mut self) {
        self.space = None;
        self.end_line();
    }

    fn picture(&mut self, image: usize, cols: u16, rows: u16) {
        if !self.line.is_empty() {
            self.space = None;
            self.end_line();
        }
        self.space = None;
        let padding = self.padding(usize::from(cols));
        self.fragment.settle();
        let line = self.fragment.lines.len();
        for _ in 0..rows {
            // Not blank, so that gaps don't merge into it.
            let row = " ".repeat(padding + usize::from(cols));
            self.fragment.lines.push(Line::raw(row));
            self.fragment.blank.push(false);
        }
        self.fragment.pictures.push(PictureSlot {
            image,
            line,
            // Narrower than the width, a u16.
            column: u16::try_from(padding).unwrap_or(0),
            cols,
            rows,
        });
    }

    fn finish(mut self) -> Fragment {
        if !self.line.is_empty()
            || (self.white_space != WhiteSpace::Collapse && self.space.is_some())
        {
            self.space = None;
            self.end_line();
        }
        self.fragment
    }
}

/// The width of `blocks`' widest word, which they can't be laid out narrower than without cutting it.
pub(crate) fn min_width(blocks: &[Block]) -> u16 {
    blocks
        .iter()
        .map(|block| match block {
            Block::Flow(flow) => flow_min_width(flow),
            Block::Stack(blocks, frame) => {
                let frame = match frame {
                    Frame::Plain => 0,
                    Frame::Quote => 2,
                    Frame::Item(marker) => u16::try_from(marker.width()).unwrap_or(u16::MAX),
                    Frame::Indent(cells) => *cells,
                };
                min_width(blocks).saturating_add(frame)
            }
            Block::Gap | Block::Rule => 0,
            Block::Table(grid) => table::min_width(grid),
        })
        .max()
        .unwrap_or(0)
}

fn flow_min_width(flow: &Flow) -> u16 {
    let mut widest = 0usize;
    let mut word = 0usize;
    for item in &flow.items {
        match item {
            Item::Text(text, _) => {
                for (index, piece) in text.split(char::is_whitespace).enumerate() {
                    if index > 0 || flow.white_space == WhiteSpace::Pre {
                        widest = widest.max(word);
                        word = 0;
                    }
                    word += piece.width();
                }
            }
            Item::Break | Item::Image(..) => {
                widest = widest.max(word);
                word = 0;
            }
        }
    }
    u16::try_from(widest.max(word)).unwrap_or(u16::MAX)
}

/// How wide `line` is, in cells.
pub(crate) fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|span| span.content.width()).sum()
}
