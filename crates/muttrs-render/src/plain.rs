//! Plain text read into blocks: quotes by depth, paragraphs, the signature, and URLs as Links.

use std::collections::HashMap;

use ratatui_core::style::Style;

use crate::ir::{Align, Block, Flow, Frame, Item, WhiteSpace};
use crate::{Document, Link, Styles, TextFormat};

pub(crate) fn read(text: &str, format: TextFormat, styles: &Styles) -> Document {
    let text = text.replace("\r\n", "\n");
    let lines = logical_lines(&text, format);
    let mut reader = Reader {
        styles,
        links: Vec::new(),
        numbers: HashMap::new(),
    };
    let blocks = reader.depth(&lines, 0);
    Document {
        blocks,
        links: reader.links,
        images: Vec::new(),
        styles: styles.clone(),
    }
}

/// A line of the body, after flowed lines are joined.
#[derive(Debug, PartialEq, Eq)]
struct Logical {
    /// How many quotes deep it is.
    depth: usize,
    text: String,
    kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    /// It introduces the quote that follows it, as "On Monday, Ann wrote:".
    Attribution,
    /// It's part of the signature, from its `-- ` line on.
    Signature,
}

/// The lines of `text`, their quote marks counted and taken off, joined where `format` flows them.
fn logical_lines(text: &str, format: TextFormat) -> Vec<Logical> {
    let mut lines: Vec<Logical> = Vec::new();
    // Whether the last line flows into the next.
    let mut flowing = false;
    let mut in_signature = false;
    for raw in text.split('\n') {
        let (depth, mut content) = unquote(raw);
        if let TextFormat::Flowed { .. } = format {
            // Space-stuffing: a line starting with a space has had one added.
            content = content.strip_prefix(' ').unwrap_or(content);
        }
        let is_separator = content == "-- " || (content == "--" && format == TextFormat::Fixed);
        if depth == 0 && is_separator {
            in_signature = true;
        }
        let kind = if in_signature && depth == 0 {
            Kind::Signature
        } else {
            Kind::Text
        };
        let flows = match format {
            TextFormat::Flowed { .. } => content.ends_with(' ') && !is_separator,
            TextFormat::Fixed => false,
        };
        let content = match format {
            TextFormat::Flowed { delete_space: true } if flows => &content[..content.len() - 1],
            _ => content,
        };
        match lines.last_mut() {
            Some(last) if flowing && last.depth == depth => last.text.push_str(content),
            _ => lines.push(Logical {
                depth,
                text: content.to_owned(),
                kind,
            }),
        }
        flowing = flows;
    }
    // A text line right before a deeper quote introduces it.
    for index in 1..lines.len() {
        let (before, after) = lines.split_at_mut(index);
        let (line, next) = (&mut before[index - 1], &after[0]);
        if next.depth > line.depth && line.kind == Kind::Text && introduces(&line.text) {
            line.kind = Kind::Attribution;
        }
    }
    while lines.last().is_some_and(|line| line.text.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// How many quotes deep `line` is, by its leading `>`s (spaces between them allowed), and what it says after them.
fn unquote(line: &str) -> (usize, &str) {
    let mut depth = 0;
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start_matches(' ');
        // Quote marks start the line, or follow others, not indented text.
        if let Some(after) = trimmed.strip_prefix('>')
            && (depth > 0 || rest.len() - trimmed.len() <= 1)
        {
            depth += 1;
            rest = after;
        } else {
            break;
        }
    }
    if depth > 0 {
        rest = rest.strip_prefix(' ').unwrap_or(rest);
    }
    (depth, rest)
}

/// Whether `line` introduces a quote, as mail clients write it in several languages.
fn introduces(line: &str) -> bool {
    let line = line.trim_end();
    [
        "wrote:",
        "écrit :",
        "écrit:",
        "schrieb:",
        "escribió:",
        "scritto:",
        "escreveu:",
    ]
    .iter()
    .any(|ending| line.ends_with(ending))
}

struct Reader<'a> {
    styles: &'a Styles,
    links: Vec<Link>,
    numbers: HashMap<String, usize>,
}

impl Reader<'_> {
    /// The blocks of `lines`, all at least `depth` quotes deep.
    fn depth(&mut self, lines: &[Logical], depth: usize) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut at = 0;
        while at < lines.len() {
            if lines[at].depth > depth {
                let end = lines[at..]
                    .iter()
                    .position(|line| line.depth <= depth)
                    .map_or(lines.len(), |offset| at + offset);
                blocks.push(Block::Gap);
                blocks.push(Block::Stack(
                    self.depth(&lines[at..end], depth + 1),
                    Frame::Quote,
                ));
                blocks.push(Block::Gap);
                at = end;
                continue;
            }
            if lines[at].text.trim().is_empty() {
                blocks.push(Block::Gap);
                at += 1;
                continue;
            }
            // A paragraph: lines at this depth up to a blank line or a quote.
            let end = lines[at..]
                .iter()
                .position(|line| line.depth != depth || line.text.trim().is_empty())
                .map_or(lines.len(), |offset| at + offset);
            let mut flow = Flow::new(Align::Left, WhiteSpace::PreWrap);
            for (index, line) in lines[at..end].iter().enumerate() {
                if index > 0 {
                    flow.items.push(Item::Break);
                }
                let style = match line.kind {
                    Kind::Text => self.styles.text,
                    Kind::Attribution => self.styles.text.patch(self.styles.attribution),
                    Kind::Signature => self.styles.text.patch(self.styles.signature),
                };
                self.linkify(&line.text, style, &mut flow);
            }
            blocks.push(Block::Flow(flow));
            at = end;
        }
        blocks
    }

    /// `text` into `flow` in `style`, its URLs in the Link style and numbered.
    fn linkify(&mut self, text: &str, style: Style, flow: &mut Flow) {
        let mut rest = text;
        while let Some((start, end)) = find_url(rest) {
            flow.push_text(&rest[..start], style);
            let url = &rest[start..end];
            if !self.numbers.contains_key(url) {
                self.links.push(Link {
                    url: url.to_owned(),
                    text: url.to_owned(),
                });
                self.numbers.insert(url.to_owned(), self.links.len());
            }
            flow.push_text(url, style.patch(self.styles.link));
            rest = &rest[end..];
        }
        flow.push_text(rest, style);
    }
}

/// Where the first URL in `text` starts and ends: `http://`, `https://` or `mailto:` up to white space or a bracket,
/// less the punctuation ending a sentence, and less a closing parenthesis it didn't open.
fn find_url(text: &str) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let start = ["https://", "http://", "mailto:"]
        .iter()
        .filter_map(|scheme| lower.find(scheme))
        .min()?;
    let length = text[start..]
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
        .unwrap_or(text.len() - start);
    let mut url = &text[start..start + length];
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '*']);
        let trimmed = if trimmed.ends_with(')')
            && trimmed.matches('(').count() < trimmed.matches(')').count()
        {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        let trimmed = trimmed.trim_end_matches([']', '}']);
        if trimmed.len() == url.len() {
            break;
        }
        url = trimmed;
    }
    let scheme_only = ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| url.eq_ignore_ascii_case(scheme));
    if scheme_only {
        return find_url(&text[start + length..])
            .map(|(s, e)| (s + start + length, e + start + length));
    }
    Some((start, start + url.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Logical]) -> Vec<(usize, &str, Kind)> {
        lines
            .iter()
            .map(|line| (line.depth, line.text.as_str(), line.kind))
            .collect()
    }

    #[test]
    fn quotes_are_counted_and_attributions_found() {
        let lines = logical_lines(
            "Sure.\n\nOn Mon, Ann wrote:\n> Lunch?\n>> said Bob\n> > nested too\n  > indented is no quote\n",
            TextFormat::Fixed,
        );
        assert_eq!(
            texts(&lines),
            [
                (0, "Sure.", Kind::Text),
                (0, "", Kind::Text),
                (0, "On Mon, Ann wrote:", Kind::Attribution),
                (1, "Lunch?", Kind::Text),
                (2, "said Bob", Kind::Text),
                (2, "nested too", Kind::Text),
                (0, "  > indented is no quote", Kind::Text),
            ]
        );
    }

    #[test]
    fn flowed_lines_join_and_stuffing_goes() {
        let lines = logical_lines(
            "A long \nline.\n> quoted \n> on.\n From here\n-- \nAnn",
            TextFormat::Flowed {
                delete_space: false,
            },
        );
        assert_eq!(
            texts(&lines),
            [
                (0, "A long line.", Kind::Text),
                (1, "quoted on.", Kind::Text),
                (0, "From here", Kind::Text),
                (0, "-- ", Kind::Signature),
                (0, "Ann", Kind::Signature),
            ]
        );
        let deleting = logical_lines("Ver \nbinding", TextFormat::Flowed { delete_space: true });
        assert_eq!(deleting[0].text, "Verbinding");
    }

    #[test]
    fn urls_end_before_punctuation_and_unopened_parentheses() {
        let found = |text: &'static str| find_url(text).map(|(s, e)| &text[s..e]);
        assert_eq!(
            found("see https://example.com/a."),
            Some("https://example.com/a")
        );
        assert_eq!(
            found("(at https://en.wikipedia.org/wiki/Rust_(language))"),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        assert_eq!(found("<https://example.com>"), Some("https://example.com"));
        assert_eq!(
            found("http:// nothing, then mailto:a@b.c!"),
            Some("mailto:a@b.c")
        );
        assert_eq!(found("no links here"), None);
    }
}
