//! Writing: a Draft is edited in `$EDITOR`, as the kit hands it the terminal, then shown as it will be sent
//! (headers, Attachments, its Markdown rendered) in a buffer of its own until it's sent, postponed or discarded.

use muttrs_mail::Message;
use muttrs_mail::compose::{Draft, Source};
use ratatui::style::Modifier;
use wcgw_kit::doc::{BlockKey, Content};
use wcgw_kit_text::{CellSize, Document, Options, Styles};

use crate::app::{Line, Span, Target};
use crate::hl::{self, Group};

/// A Draft being written, for an Account.
pub(crate) struct Compose {
    /// The Account it's sent from, by its place in the sidebar.
    pub(crate) account: usize,
    pub(crate) draft: Draft,
    /// The Message it answers, by its Mailbox and UID, to flag answered once it's sent.
    pub(crate) answers: Option<(String, u32)>,
    /// The Postponed Draft it was resumed from, by its Mailbox and UID, expunged once it's sent or kept again.
    pub(crate) replaces: Option<(String, u32)>,
    /// What the editor was first given, until the Draft's edited.
    unedited: Option<String>,
    /// Whether the editor changed it once: it's a Draft then, whatever the editor does after.
    edited: bool,
}

impl Compose {
    /// A Draft to write.
    pub(crate) fn new(account: usize, draft: Draft, answers: Option<(String, u32)>) -> Self {
        Self {
            account,
            draft,
            answers,
            replaces: None,
            unedited: None,
            edited: false,
        }
    }

    /// The text to edit it as: headers, a blank line, then the body.
    pub(crate) fn text(&mut self) -> String {
        let text = self.draft.to_text();
        if !self.edited && self.unedited.is_none() {
            self.unedited = Some(text.clone());
        }
        text
    }

    /// Whether the editor changed it once: a Draft then, whatever the editor does after.
    pub(crate) fn was_edited(&self) -> bool {
        self.edited
    }

    /// Takes the text the editor saved: `false` if it's as it was before it was first edited, as when the editor
    /// quit without saving, which drops a new Draft.
    ///
    /// # Errors
    ///
    /// If its headers don't say what muttrs understands.
    pub(crate) fn saved(&mut self, text: &str) -> anyhow::Result<bool> {
        if !self.edited && self.unedited.as_deref() == Some(text) {
            return Ok(false);
        }
        self.draft = self.draft.edited(text, crate::xdg::home().as_deref())?;
        self.edited = true;
        self.unedited = None;
        Ok(true)
    }

    /// The Draft as it will be sent, as blocks of a kit document `width` cells wide, its body drawn in `styles`:
    /// its headers, which open it in the editor again, and its Markdown rendered.
    pub(crate) fn blocks(
        &self,
        width: u16,
        styles: &Styles,
        attachment: &str,
    ) -> Vec<(BlockKey, Content<Target>)> {
        let mut headers = self.headers(attachment);
        headers.push(Line::Text(vec![Span::new(
            "─".repeat(usize::from(width)),
            hl::DIM,
        )]));
        let body = Document::html(&muttrs_mail::compose::html(&self.draft.body), styles);
        let rendering = body.render(&Options {
            width,
            cell: CellSize::default(),
            pixel_ratio: 1.0,
            max_picture_rows: 0,
            pictures: &[],
        });
        let body = crate::pager::body_lines(&rendering, body.links(), styles, |_| String::new());
        vec![
            (
                BlockKey::new("headers"),
                Content {
                    lines: headers,
                    folds: Vec::new(),
                    target: Some(Target::Draft),
                },
            ),
            (
                BlockKey::new("body"),
                Content {
                    lines: body,
                    folds: Vec::new(),
                    target: Some(Target::Draft),
                },
            ),
        ]
    }

    fn headers(&self, attachment: &str) -> Vec<Line> {
        let draft = &self.draft;
        let label = |name: &str| Span::new(format!("{name:<9}"), hl::DIM);
        let list = |addresses: &[muttrs_mail::Address]| {
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines = vec![Line::Text(vec![
            label("From"),
            Span::new(
                draft.from.as_ref().map_or_default(ToString::to_string),
                Group::NORMAL,
            ),
        ])];
        for (name, addresses) in [("To", &draft.to), ("Cc", &draft.cc), ("Bcc", &draft.bcc)] {
            if !addresses.is_empty() || name == "To" {
                let value = if addresses.is_empty() {
                    Span::new("nobody yet: <CR> edits the Draft", hl::ERROR)
                } else {
                    Span::new(list(addresses), Group::NORMAL)
                };
                lines.push(Line::Text(vec![label(name), value]));
            }
        }
        lines.push(Line::Text(vec![
            label("Subject"),
            Span::new(
                if draft.subject.is_empty() {
                    "(no subject)".to_owned()
                } else {
                    draft.subject.clone()
                },
                hl::SUBJECT,
            ),
        ]));
        for attached in &draft.attachments {
            let size = match &attached.source {
                Source::File(path) => match std::fs::metadata(path) {
                    Ok(metadata) => usize::try_from(metadata.len())
                        .map_or_else(|_| String::new(), crate::pager::size),
                    Err(_) => "missing".to_owned(),
                },
                Source::Bytes(bytes) => crate::pager::size(bytes.len()),
            };
            lines.push(Line::Text(vec![
                Span::new(format!("{:<9}", attachment.trim_end()), hl::ATTACHMENT),
                Span::new(format!("{} ({size})", attached.name), hl::ATTACHMENT),
            ]));
        }
        lines
    }
}

/// The text of `message` to quote in a reply or forward: its plain body, or else its HTML body laid out as text,
/// without the numbers after its Links.
pub(crate) fn quotable(message: &Message) -> String {
    if let Some((text, format)) = message.plain() {
        let text = text.replace("\r\n", "\n");
        return match format {
            muttrs_mail::TextFormat::Flowed { delete_space } => unflow(&text, delete_space),
            muttrs_mail::TextFormat::Fixed => text,
        };
    }
    let Some(html) = message.html() else {
        return String::new();
    };
    // The Links' numbers drawn hidden, to be left out.
    let styles = Styles {
        link_number: ratatui::style::Style::new().add_modifier(Modifier::HIDDEN),
        ..Styles::default()
    };
    let rendering = Document::html(&html, &styles).render(&Options {
        width: 72,
        cell: CellSize::default(),
        pixel_ratio: 1.0,
        max_picture_rows: 0,
        pictures: &[],
    });
    rendering
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .filter(|span| !span.style.add_modifier.contains(Modifier::HIDDEN))
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text`, written `format=flowed`, with its flowed lines joined: each paragraph on one line, as it was meant.
fn unflow(text: &str, delete_space: bool) -> String {
    let depth = |line: &str| line.chars().take_while(|&c| c == '>').count();
    let mut out = String::with_capacity(text.len());
    let mut flowing: Option<usize> = None;
    for line in text.lines() {
        let line_depth = depth(line);
        let content = line[line_depth..]
            .strip_prefix(' ')
            .unwrap_or(&line[line_depth..]);
        match flowing {
            Some(previous) if previous == line_depth => {
                if delete_space {
                    out.pop();
                }
                out.push_str(content);
            }
            _ => {
                if flowing.is_some() {
                    out.push('\n');
                }
                out.push_str(&line[..line_depth]);
                if line_depth > 0 {
                    out.push(' ');
                }
                out.push_str(content);
            }
        }
        let flows = line.ends_with(' ') && content != "-- ";
        if flows {
            flowing = Some(line_depth);
        } else {
            flowing = None;
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_draft_left_as_it_was_is_dropped_and_edited_once_it_is_not_compared() {
        let mut compose = Compose::new(0, Draft::default(), None);
        let text = compose.text();
        assert!(!compose.saved(&text).unwrap(), "as it was");
        let mut compose = Compose::new(0, Draft::default(), None);
        let text = compose.text();
        let edited = text.replace("Subject:", "Subject: Lunch");
        assert!(compose.saved(&edited).unwrap());
        assert_eq!(compose.draft.subject, "Lunch");
        let again = compose.text();
        assert!(
            compose.saved(&again).unwrap(),
            "edited again, unchanged: still a Draft"
        );
    }
    #[test]
    fn html_only_messages_quote_as_text_without_link_numbers() {
        let message = Message::parse(
            b"Subject: hi\r\nContent-Type: text/html\r\n\r\n<p>See <a href=\"https://x.example\">this</a>.</p><p>Bye</p>"
                .to_vec(),
        )
        .unwrap();
        assert_eq!(quotable(&message), "See this.\n\nBye");
    }

    #[test]
    fn flowed_text_is_joined_before_it_is_quoted() {
        assert_eq!(
            unflow("This is \nflowed.\n> and so \n> is this\n-- \nAnn\n", false),
            "This is flowed.\n> and so is this\n-- \nAnn\n"
        );
        assert_eq!(unflow("Ver \nbinding\n", true), "Verbinding\n");
    }
}
