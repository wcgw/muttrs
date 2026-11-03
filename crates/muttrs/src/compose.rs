//! Writing: a Draft is edited as a file in `$EDITOR`, then shown as it will be sent (headers, Attachments, its
//! Markdown rendered) until it's sent, postponed or discarded.

use std::path::{Path, PathBuf};

use muttrs_mail::Message;
use muttrs_mail::compose::{Draft, Source};
use muttrs_render::{CellSize, Document, Options, Styles};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::icons::Icons;
use crate::theme::Theme;
use crate::xdg;

/// A Draft being written, for an Account.
pub(crate) struct Compose {
    /// The Account it's sent from, by its place in the sidebar.
    pub(crate) account: usize,
    pub(crate) draft: Draft,
    /// The file it's edited in.
    pub(crate) path: PathBuf,
    /// The Message it answers, by its Mailbox and UID, to flag answered once it's sent.
    pub(crate) answers: Option<(String, u32)>,
    /// The Postponed Draft it was resumed from, by its Mailbox and UID, expunged once it's sent or kept again.
    pub(crate) replaces: Option<(String, u32)>,
    /// Whether discarding it was asked for once, and needs asking again.
    pub(crate) discarding: bool,
    /// What its file said before it was first edited, while it's not been edited yet.
    unedited: Option<String>,
    scroll: usize,
    height: usize,
}

impl Compose {
    /// A Draft to write, in a file of its own, numbered `serial` among this run's.
    pub(crate) fn new(
        account: usize,
        draft: Draft,
        answers: Option<(String, u32)>,
        serial: u64,
    ) -> std::io::Result<Self> {
        let directory = xdg::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("drafts");
        create_private(&directory)?;
        let path = directory.join(format!("muttrs-{}-{serial}.eml", std::process::id()));
        let text = draft.to_text();
        write_private(&path, text.as_bytes())?;
        Ok(Self {
            unedited: Some(text),
            account,
            draft,
            path,
            answers,
            replaces: None,
            discarding: false,
            scroll: 0,
            height: 0,
        })
    }

    /// Reads the file back, as `$EDITOR` left it: `false` if it's as it was before it was first edited, as when
    /// the editor quit without saving, which drops a new Draft.
    pub(crate) fn reread(&mut self) -> anyhow::Result<bool> {
        let text = std::fs::read_to_string(&self.path)?;
        if self
            .unedited
            .take()
            .is_some_and(|unedited| unedited == text)
        {
            return Ok(false);
        }
        self.draft = self.draft.edited(&text, xdg::home().as_deref())?;
        Ok(true)
    }

    /// Writes the Draft to its file again, as after attaching.
    pub(crate) fn rewrite(&self) -> std::io::Result<()> {
        write_private(&self.path, self.draft.to_text().as_bytes())
    }

    /// Deletes its file: it's sent, postponed or discarded.
    pub(crate) fn finish(self) {
        let _ = std::fs::remove_file(&self.path);
    }

    pub(crate) fn scroll_by(&mut self, lines: isize) {
        self.scroll = self.scroll.saturating_add_signed(lines);
    }

    pub(crate) fn draw(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        icons: &Icons,
        pager_width: u16,
    ) {
        if area.width < 4 || area.height == 0 {
            return;
        }
        let width = (area.width - 2).min(pager_width);
        let mut lines = self.headers(theme, icons);
        lines.push(Line::styled("─".repeat(usize::from(width)), theme.dim));
        let body = Document::html(&muttrs_mail::compose::html(&self.draft.body), &theme.body);
        lines.extend(
            body.render(&Options {
                width,
                cell: CellSize::default(),
                pixel_ratio: 1.0,
                max_picture_rows: 0,
                pictures: &[],
            })
            .lines,
        );
        self.height = usize::from(area.height);
        self.scroll = self.scroll.min(lines.len().saturating_sub(self.height));
        for (row, line) in lines.iter().skip(self.scroll).take(self.height).enumerate() {
            buffer.set_line(
                area.x + 1,
                area.y + u16::try_from(row).unwrap_or(0),
                line,
                width,
            );
        }
    }

    fn headers(&self, theme: &Theme, icons: &Icons) -> Vec<Line<'static>> {
        let draft = &self.draft;
        let label = |name: &str| Span::styled(format!("{name:<9}"), theme.dim);
        let list = |addresses: &[muttrs_mail::Address]| {
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut lines = vec![Line::from(vec![
            label("From"),
            Span::raw(
                draft
                    .from
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            ),
        ])];
        for (name, addresses) in [("To", &draft.to), ("Cc", &draft.cc), ("Bcc", &draft.bcc)] {
            if !addresses.is_empty() || name == "To" {
                let value = if addresses.is_empty() {
                    Span::styled("nobody yet: e edits the Draft", theme.error)
                } else {
                    Span::raw(list(addresses))
                };
                lines.push(Line::from(vec![label(name), value]));
            }
        }
        lines.push(Line::from(vec![
            label("Subject"),
            Span::styled(
                if draft.subject.is_empty() {
                    "(no subject)".to_owned()
                } else {
                    draft.subject.clone()
                },
                theme.subject,
            ),
        ]));
        for attachment in &draft.attachments {
            let size = match &attachment.source {
                Source::File(path) => match std::fs::metadata(path) {
                    Ok(metadata) => usize::try_from(metadata.len())
                        .map_or_else(|_| String::new(), crate::pager::size),
                    Err(_) => "missing".to_owned(),
                },
                Source::Bytes(bytes) => crate::pager::size(bytes.len()),
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<9}", icons.attachment.trim_end()),
                    theme.attachment,
                ),
                Span::styled(format!("{} ({size})", attachment.name), theme.attachment),
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

/// The editor to write in: the configuration's, `$VISUAL`, `$EDITOR`, or `vi`.
pub(crate) fn editor(configured: Option<&str>) -> String {
    configured
        .map(str::to_owned)
        .or_else(|| {
            std::env::var("VISUAL")
                .ok()
                .filter(|editor| !editor.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|editor| !editor.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_owned())
}

/// Runs `editor` on `path`, through `sh` so that an editor given with arguments (`code --wait`) works, and waits
/// for it.
pub(crate) fn edit(editor: &str, path: &Path) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("muttrs")
        .arg(path)
        .status()
}

fn create_private(directory: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(directory)
}

/// Writes `bytes` to `path`, readable by the user only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn editors_with_arguments_get_the_path_last() {
        let path = std::env::temp_dir().join(format!("muttrs-edit-{}", std::process::id()));
        std::fs::write(&path, "before").unwrap();
        // An "editor" that appends to the file, given with an argument of its own.
        let status = edit("sh -c 'echo after >> \"$0\"'", &path).unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "beforeafter\n");
        std::fs::remove_file(path).unwrap();
    }
}
