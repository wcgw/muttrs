//! What a body is read into, HTML and plain text alike, before it's laid out for a width: blocks stacked on top of
//! each other, and flows of text that wrap.

use ratatui_core::style::Style;

/// A piece of a body that takes whole lines.
#[derive(Clone, Debug)]
pub(crate) enum Block {
    /// Text and images that wrap into lines.
    Flow(Flow),
    /// Blocks drawn within a frame: a quote's bar, a list item's marker, an indent.
    Stack(Vec<Block>, Frame),
    /// Space between blocks: one blank line, however many gaps meet.
    Gap,
    /// A horizontal rule across the width.
    Rule,
    /// A table of data, drawn as a grid.
    Table(Table),
}

/// What surrounds the blocks of a [`Block::Stack`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// Nothing: a `<div>`.
    Plain,
    /// A quote's bar down the left, one more level deep.
    Quote,
    /// A list item: its marker on the first line, the rest indented as far.
    Item(String),
    /// Indented by this many cells, as a `<dd>`.
    Indent(u16),
}

/// Text that wraps, as a paragraph.
#[derive(Clone, Debug, Default)]
pub(crate) struct Flow {
    pub(crate) items: Vec<Item>,
    pub(crate) align: Align,
    pub(crate) white_space: WhiteSpace,
}

impl Flow {
    pub(crate) fn new(align: Align, white_space: WhiteSpace) -> Self {
        Self {
            items: Vec::new(),
            align,
            white_space,
        }
    }

    pub(crate) fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if let Some(Item::Text(last, last_style)) = self.items.last_mut()
            && *last_style == style
        {
            last.push_str(text);
            return;
        }
        self.items.push(Item::Text(text.to_owned(), style));
    }
}

/// A piece of a [`Flow`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    Text(String, Style),
    /// A line break: `<br>`, or a new line in plain text.
    Break,
    /// The image at this index of the document's: a Picture of its own lines if it can be drawn, else its
    /// description, in this style.
    Image(usize, Style),
}

/// How a [`Flow`]'s lines are placed across the width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// What a [`Flow`] does with its white space, after CSS's `white-space`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WhiteSpace {
    /// Runs of white space are one space, and lines wrap: HTML's text.
    #[default]
    Collapse,
    /// White space is kept, new lines break, and lines wrap: plain text.
    PreWrap,
    /// White space is kept, new lines break, and lines don't wrap but are cut: `<pre>`.
    Pre,
}

/// A table of data.
#[derive(Clone, Debug, Default)]
pub(crate) struct Table {
    pub(crate) rows: Vec<Row>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Row {
    pub(crate) cells: Vec<Cell>,
    /// Whether this row heads the table: in `<thead>`, or of header cells alone.
    pub(crate) header: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Cell {
    pub(crate) blocks: Vec<Block>,
    /// How many columns it spans, at least one.
    pub(crate) span: usize,
}
