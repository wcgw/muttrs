//! A body's prose: the text of its paragraphs, for voight to assess, without what it quotes.

use crate::ir::{Block, Flow, Frame, Item, WhiteSpace};

/// The prose of `blocks`: each paragraph's text, apart by blank lines, as it's drawn but not wrapped. What's quoted
/// (someone else's, mostly), preformatted (code, mostly) and images are left out.
pub(crate) fn of(blocks: &[Block]) -> String {
    let mut prose = String::new();
    add(blocks, &mut prose);
    prose
}

fn add(blocks: &[Block], prose: &mut String) {
    for block in blocks {
        match block {
            Block::Flow(flow) if flow.white_space != WhiteSpace::Pre => paragraph(flow, prose),
            Block::Flow(_) | Block::Gap | Block::Rule | Block::Stack(_, Frame::Quote) => {}
            Block::Stack(blocks, _) => add(blocks, prose),
            Block::Table(table) => {
                for cell in table.rows.iter().flat_map(|row| &row.cells) {
                    add(&cell.blocks, prose);
                }
            }
        }
    }
}

fn paragraph(flow: &Flow, prose: &mut String) {
    let mut text = String::new();
    for item in &flow.items {
        match item {
            Item::Text(piece, _) => text.push_str(piece),
            Item::Break => text.push('\n'),
            Item::Image(..) => {}
        }
    }
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !prose.is_empty() {
        prose.push_str("\n\n");
    }
    prose.push_str(text);
}

#[cfg(test)]
mod tests {
    use crate::{Document, Styles, TextFormat};

    #[test]
    fn plain_prose_leaves_quotes_out() {
        let text = "Thursday works.\nSee you at noon.\n\nOn Monday, Ann wrote:\n> Lunch on Thursday?\n>> Earlier\n\n-- \nAlex\n";
        let document = Document::plain(text, TextFormat::Fixed, &Styles::default());
        assert_eq!(
            document.prose(),
            "Thursday works.\nSee you at noon.\n\nOn Monday, Ann wrote:\n\n-- \nAlex"
        );
    }

    #[test]
    fn html_prose_is_its_paragraphs_text_without_quotes_or_code() {
        let html = "<p>One   paragraph,\n with <b>bold</b> and <a href=\"https://x.y\">a link</a>.</p>\
                    <blockquote><p>Quoted.</p></blockquote>\
                    <ul><li>An item</li></ul><pre>let code = 1;</pre>\
                    <table><tr><td>A cell</td></tr></table><img alt=\"a picture\" src=\"cid:x\">";
        let document = Document::html(html, &Styles::default());
        assert_eq!(
            document.prose(),
            "One paragraph, with bold and a link[1].\n\nAn item\n\nA cell"
        );
    }
}
