//! Drafts: written as text in `$EDITOR` (headers, a blank line, a body in Markdown), read back, and made into the
//! Message sent: the Markdown as plain text and as the HTML it makes, with the Attachments.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use lettre::message::header::ContentType;
use lettre::message::{Attachment as Part, Mailbox, MultiPart};
use mail_parser::MessageParser;

use crate::message::{self, Address, Message};

/// A Message being written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    /// Who it's from: the Account's address, unless the Draft says otherwise.
    pub from: Option<Address>,
    /// Who it's to.
    pub to: Vec<Address>,
    /// Who it's copied to.
    pub cc: Vec<Address>,
    /// Who it's copied to without the others knowing.
    pub bcc: Vec<Address>,
    /// Its subject, maybe empty.
    pub subject: String,
    /// The `Message-ID` it replies to.
    pub in_reply_to: Option<String>,
    /// The `Message-ID`s of the Messages before it in its thread.
    pub references: Vec<String>,
    /// What it says, in Markdown.
    pub body: String,
    /// The files going with it.
    pub attachments: Vec<Attachment>,
}

/// A file going with a Draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// The file name it's sent under.
    pub name: String,
    /// Its MIME type, as `application/pdf`.
    pub mime_type: String,
    /// Where its bytes are.
    pub source: Source,
}

/// Where an Attachment's bytes are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file, read when the Draft is sent: one named on an `Attach:` line.
    File(PathBuf),
    /// Bytes muttrs has: a forwarded Message's Attachments.
    Bytes(Arc<Vec<u8>>),
}

impl Attachment {
    /// The file at `path`, its type guessed from its extension.
    #[must_use]
    pub fn file(path: PathBuf) -> Self {
        let name = path.file_name().map_or_else(
            || "attachment".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self {
            mime_type: guess_type(&name).to_owned(),
            name,
            source: Source::File(path),
        }
    }
}

/// Why a Draft can't be read, or made into a Message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A header names something that isn't an address.
    #[error("{0:?} isn't an address")]
    Address(String),
    /// The Draft is to no one: no `To`, `Cc` or `Bcc`.
    #[error("the Draft has no recipient")]
    NoRecipient,
    /// The Draft's `From` is empty.
    #[error("the Draft says nothing it's from")]
    NoSender,
    /// A line among the headers, by its number, isn't one muttrs knows.
    #[error(
        "line {0}, {1:?}, isn't a header: headers come first, then a blank line, then the body"
    )]
    Header(usize, String),
    /// A file to attach can't be read.
    #[error("reading the Attachment {0}: {1}")]
    Attachment(String, std::io::Error),
    /// The Message can't be made, as lettre says.
    #[error("making the Message: {0}")]
    Build(String),
}

/// A Draft made into a Message, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// The Message as sent: without its `Bcc`.
    pub message: Vec<u8>,
    /// The same Message with its `Bcc`, as kept in Sent.
    pub kept: Vec<u8>,
    /// The address it's sent from, for SMTP's envelope.
    pub from: String,
    /// Every recipient, `Bcc` included, for SMTP's envelope.
    pub recipients: Vec<String>,
}

impl Draft {
    /// A new Message from `from`.
    #[must_use]
    pub fn new(from: Address) -> Self {
        Self {
            from: Some(from),
            ..Self::default()
        }
    }

    /// A reply to `original`, from `me`, to its sender or, `all`, to everyone it went to but `me`; `quoted` is its
    /// text, to quote under an attribution.
    #[must_use]
    pub fn reply(original: &Message, me: Address, all: bool, quoted: &str) -> Self {
        let senders = if original.reply_to.is_empty() {
            original.from.clone()
        } else {
            original.reply_to.clone()
        };
        let mut to = senders;
        let mut cc = Vec::new();
        if all {
            to.extend(original.to.iter().cloned());
            cc.extend(original.cc.iter().cloned());
        }
        let is_me = |address: &Address| address.email.eq_ignore_ascii_case(&me.email);
        let mut seen: Vec<String> = Vec::new();
        let mut keep = |address: &Address| {
            let email = address.email.to_ascii_lowercase();
            let new = !seen.contains(&email);
            seen.push(email);
            new
        };
        let mut to: Vec<Address> = to.into_iter().filter(|a| !is_me(a) && keep(a)).collect();
        let cc: Vec<Address> = cc.into_iter().filter(|a| !is_me(a) && keep(a)).collect();
        if to.is_empty() {
            // A reply to one's own Message goes where it went.
            to.clone_from(&original.to);
        }
        let mut references = original.references.clone();
        references.extend(original.message_id.clone());
        Self {
            from: Some(me),
            to,
            cc,
            bcc: Vec::new(),
            subject: prefixed("Re:", &original.subject),
            in_reply_to: original.message_id.clone(),
            references,
            body: format!("\n\n{}\n{}", attribution(original), quote(quoted)),
            attachments: Vec::new(),
        }
    }

    /// A Postponed Draft, as it was kept: its headers, its body (`text`, its Markdown) and its Attachments.
    #[must_use]
    pub fn resumed(kept: &Message, text: &str) -> Self {
        Self {
            from: kept.from.first().cloned(),
            to: kept.to.clone(),
            cc: kept.cc.clone(),
            bcc: kept.bcc.clone(),
            subject: kept.subject.clone(),
            in_reply_to: kept.in_reply_to.first().cloned(),
            references: kept.references.clone(),
            body: text.replace("\r\n", "\n"),
            attachments: kept
                .parts()
                .iter()
                .filter(|part| !part.inline || part.content_id.is_none())
                .map(|part| Attachment {
                    name: part.file_name(),
                    mime_type: part.mime_type.clone(),
                    source: Source::Bytes(Arc::new(kept.bytes(part))),
                })
                .collect(),
        }
    }

    /// `original` forwarded, from `me`: its text after its headers, and its Attachments.
    #[must_use]
    pub fn forward(original: &Message, me: Address, text: &str) -> Self {
        let list = |addresses: &[Address]| {
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut header = String::from("---------- Forwarded message ---------\n");
        // Writing to a String can't fail.
        let _ = writeln!(header, "From: {}", list(&original.from));
        if let Some(date) = original.date {
            let _ = writeln!(header, "Date: {}", long_date(date));
        }
        let _ = writeln!(header, "Subject: {}", original.subject);
        let _ = writeln!(header, "To: {}", list(&original.to));
        if !original.cc.is_empty() {
            let _ = writeln!(header, "Cc: {}", list(&original.cc));
        }
        let attachments = original
            .parts()
            .iter()
            .filter(|part| !part.inline || part.content_id.is_none())
            .map(|part| Attachment {
                name: part.file_name(),
                mime_type: part.mime_type.clone(),
                source: Source::Bytes(Arc::new(original.bytes(part))),
            })
            .collect();
        Self {
            from: Some(me),
            subject: prefixed("Fwd:", &original.subject),
            body: format!("\n\n{header}\n{}", text.trim_end()),
            attachments,
            ..Self::default()
        }
    }

    /// The Draft as text for `$EDITOR`: its headers, a blank line, its body. Attachments from files are `Attach:`
    /// lines; others aren't in the text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let list = |addresses: &[Address]| {
            addresses
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut text = String::new();
        // Writing to a String can't fail.
        let from = self
            .from
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        let _ = writeln!(text, "From: {from}");
        let _ = writeln!(text, "To: {}", list(&self.to));
        let _ = writeln!(text, "Cc: {}", list(&self.cc));
        let _ = writeln!(text, "Bcc: {}", list(&self.bcc));
        let _ = writeln!(text, "Subject: {}", self.subject);
        for attachment in &self.attachments {
            if let Source::File(path) = &attachment.source {
                let _ = writeln!(text, "Attach: {}", path.display());
            }
        }
        text.push('\n');
        text.push_str(&self.body);
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    /// Reads back the text [`Draft::to_text`] made, as edited: what isn't in the text (the Messages it replies to,
    /// Attachments not from files) is kept from `self`. `~` in `Attach:` paths is `home`.
    ///
    /// # Errors
    ///
    /// If a header isn't one, or an address isn't.
    pub fn edited(&self, text: &str, home: Option<&std::path::Path>) -> Result<Self, Error> {
        let mut draft = Self {
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            body: String::new(),
            attachments: self
                .attachments
                .iter()
                .filter(|attachment| matches!(attachment.source, Source::Bytes(_)))
                .cloned()
                .collect(),
        };
        let mut lines = text.split_inclusive('\n').enumerate();
        // Headers, with folded lines joined.
        let mut headers: Vec<(usize, String)> = Vec::new();
        for (number, line) in lines.by_ref() {
            let line = line.trim_end_matches(['\n', '\r']);
            if line.trim().is_empty() {
                break;
            }
            match headers.last_mut() {
                Some((_, last)) if line.starts_with([' ', '\t']) => {
                    last.push(' ');
                    last.push_str(line.trim());
                }
                _ => headers.push((number + 1, line.to_owned())),
            }
        }
        draft.body = lines.map(|(_, line)| line).collect::<String>();
        for (number, header) in headers {
            let Some((name, value)) = header.split_once(':') else {
                return Err(Error::Header(number, header));
            };
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "from" => draft.from = addresses(value)?.into_iter().next(),
                "to" => draft.to = addresses(value)?,
                "cc" => draft.cc = addresses(value)?,
                "bcc" => draft.bcc = addresses(value)?,
                "subject" => value.clone_into(&mut draft.subject),
                "attach" if !value.is_empty() => {
                    let path = match (value.strip_prefix("~/"), home) {
                        (Some(rest), Some(home)) => home.join(rest),
                        _ => PathBuf::from(value),
                    };
                    draft.attachments.push(Attachment::file(path));
                }
                "attach" => {}
                _ => return Err(Error::Header(number, header)),
            }
        }
        Ok(draft)
    }

    /// Whether it says anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.body.trim().is_empty() && self.attachments.is_empty()
    }

    /// The Message to send: its body as plain text and as the HTML its Markdown makes, and its Attachments, under
    /// `message_id` (with its angle brackets).
    ///
    /// # Errors
    ///
    /// If it has no sender or recipient, an address isn't one, or an Attachment can't be read.
    ///
    /// # Panics
    ///
    /// Never: `application/octet-stream`, which an Attachment of a type lettre doesn't take is sent as, parses.
    pub fn build(&self, message_id: &str) -> Result<Built, Error> {
        let from = self.from.as_ref().ok_or(Error::NoSender)?;
        if self.to.is_empty() && self.cc.is_empty() && self.bcc.is_empty() {
            return Err(Error::NoRecipient);
        }
        let mut attachments = Vec::new();
        for attachment in &self.attachments {
            let bytes = match &attachment.source {
                Source::File(path) => std::fs::read(path)
                    .map_err(|error| Error::Attachment(path.display().to_string(), error))?,
                Source::Bytes(bytes) => bytes.as_ref().clone(),
            };
            let kind = ContentType::parse(&attachment.mime_type)
                .unwrap_or(ContentType::parse("application/octet-stream").expect("a valid type"));
            attachments.push(Part::new(attachment.name.clone()).body(bytes, kind));
        }
        let build = |keep_bcc: bool| -> Result<Vec<u8>, Error> {
            let mut builder = lettre::Message::builder()
                .from(mailbox(from)?)
                .subject(self.subject.clone())
                .message_id(Some(message_id.to_owned()))
                .user_agent(format!("muttrs {}", env!("CARGO_PKG_VERSION")))
                .date_now();
            for to in &self.to {
                builder = builder.to(mailbox(to)?);
            }
            for cc in &self.cc {
                builder = builder.cc(mailbox(cc)?);
            }
            for bcc in &self.bcc {
                builder = builder.bcc(mailbox(bcc)?);
            }
            if keep_bcc {
                builder = builder.keep_bcc();
            }
            if let Some(id) = &self.in_reply_to {
                builder = builder.in_reply_to(format!("<{id}>"));
            }
            if !self.references.is_empty() {
                let references: Vec<String> =
                    self.references.iter().map(|id| format!("<{id}>")).collect();
                builder = builder.references(references.join(" "));
            }
            let body = MultiPart::alternative_plain_html(self.body.clone(), html(&self.body));
            let message = if attachments.is_empty() {
                builder.multipart(body)
            } else {
                let mixed = attachments
                    .iter()
                    .cloned()
                    .fold(MultiPart::mixed().multipart(body), MultiPart::singlepart);
                builder.multipart(mixed)
            }
            .map_err(|error| Error::Build(error.to_string()))?;
            Ok(message.formatted())
        };
        Ok(Built {
            message: build(false)?,
            kept: build(true)?,
            from: from.email.clone(),
            recipients: self
                .to
                .iter()
                .chain(&self.cc)
                .chain(&self.bcc)
                .map(|address| address.email.clone())
                .collect(),
        })
    }
}

/// A fresh `Message-ID`, angle brackets and all, at the domain of `email`.
///
/// # Panics
///
/// If the system has no random numbers to give.
#[must_use]
pub fn message_id(email: &str) -> String {
    let domain = email
        .rsplit_once('@')
        .map_or("muttrs.invalid", |(_, domain)| domain);
    let mut random = [0u8; 12];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut random)
        .expect("random numbers");
    let random = random.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    let now = Timestamp::now().as_second();
    format!("<{now}.{random}@{domain}>")
}

/// The HTML `markdown` makes, as a document: what's sent besides the Markdown itself.
#[must_use]
pub fn html(markdown: &str) -> String {
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_TABLES);
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    let parser = pulldown_cmark::Parser::new_ext(markdown, options).map(|event| match event {
        // Raw HTML in a Draft is text: nothing typed should become markup no one meant.
        pulldown_cmark::Event::Html(raw) | pulldown_cmark::Event::InlineHtml(raw) => {
            pulldown_cmark::Event::Text(raw)
        }
        // A new line breaks the line, as in plain text mail (and on GitHub): signatures, addresses and quoted
        // text keep their lines.
        pulldown_cmark::Event::SoftBreak => pulldown_cmark::Event::HardBreak,
        other => other,
    });
    let mut body = String::new();
    pulldown_cmark::html::push_html(&mut body, parser);
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body>\n{body}</body></html>\n"
    )
}

fn mailbox(address: &Address) -> Result<Mailbox, Error> {
    let email = address
        .email
        .parse()
        .map_err(|_| Error::Address(address.email.clone()))?;
    Ok(Mailbox::new(
        address.name.clone().filter(|name| !name.trim().is_empty()),
        email,
    ))
}

/// The addresses of a header's value, as typed: `Ann <ann@example.com>, bob@example.com`.
fn addresses(value: &str) -> Result<Vec<Address>, Error> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let header = format!("To: {value}\r\n\r\n");
    let parsed = MessageParser::new()
        .parse_headers(header.as_bytes())
        .ok_or_else(|| Error::Address(value.to_owned()))?;
    let found = message::addresses(parsed.root().to());
    if found.is_empty() || found.iter().any(|address| !address.email.contains('@')) {
        return Err(Error::Address(value.to_owned()));
    }
    Ok(found)
}

/// `subject` after `prefix` (as `Re:`), unless it already starts with it.
fn prefixed(prefix: &str, subject: &str) -> String {
    if subject
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    {
        subject.to_owned()
    } else {
        format!("{prefix} {subject}")
    }
}

/// "On Mon 2 Nov 2026 at 09:30, Ann <ann@example.com> wrote:".
fn attribution(original: &Message) -> String {
    let who = original
        .from
        .first()
        .map_or_else(|| "someone".to_owned(), ToString::to_string);
    match original.date {
        Some(date) => format!("On {}, {who} wrote:", long_date(date)),
        None => format!("{who} wrote:"),
    }
}

fn long_date(date: Timestamp) -> String {
    date.to_zoned(TimeZone::system())
        .strftime("%a %-d %b %Y at %H:%M")
        .to_string()
}

/// `text` quoted: each line after `> `, or `>` alone where it's blank.
fn quote(text: &str) -> String {
    text.trim_end()
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">\n".to_owned()
            } else if line.starts_with('>') {
                format!(">{line}\n")
            } else {
                format!("> {line}\n")
            }
        })
        .collect()
}

/// A MIME type for a file called `name`, by its extension.
fn guess_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "eml" => "message/rfc822",
        "ics" => "text/calendar",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(name: &str, email: &str) -> Address {
        Address {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    fn original() -> Message {
        Message::parse(
            b"From: Ann <ann@example.com>\r\nTo: Me <me@example.com>, Bob <bob@example.com>\r\n\
              Cc: carol@example.com\r\nSubject: Lunch\r\nMessage-ID: <two@example.com>\r\n\
              References: <one@example.com>\r\nDate: Mon, 2 Nov 2026 09:30:00 +0000\r\n\r\nThursday?\r\n"
                .to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn replies_go_to_the_sender_or_everyone_but_me() {
        let me = address("Me", "ME@example.com");
        let reply = Draft::reply(&original(), me.clone(), false, "Thursday?");
        assert_eq!(reply.to, [address("Ann", "ann@example.com")]);
        assert_eq!(reply.cc, []);
        assert_eq!(reply.subject, "Re: Lunch");
        assert_eq!(reply.in_reply_to.as_deref(), Some("two@example.com"));
        assert_eq!(reply.references, ["one@example.com", "two@example.com"]);
        assert!(
            reply.body.ends_with("wrote:\n> Thursday?\n"),
            "{}",
            reply.body
        );

        let all = Draft::reply(&original(), me, true, "");
        let to: Vec<&str> = all.to.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(to, ["ann@example.com", "bob@example.com"]);
        assert_eq!(all.cc[0].email, "carol@example.com");
        assert_eq!(prefixed("Re:", "RE: Lunch"), "RE: Lunch");
    }

    #[test]
    fn drafts_round_trip_through_text() {
        let mut draft = Draft::reply(
            &original(),
            address("Me", "me@example.com"),
            true,
            "Thursday?",
        );
        draft
            .attachments
            .push(Attachment::file(PathBuf::from("/tmp/plan.pdf")));
        let text = draft.to_text();
        assert!(text.starts_with(
            "From: Me <me@example.com>\nTo: Ann <ann@example.com>, Bob <bob@example.com>\n"
        ));
        assert!(text.contains("\nAttach: /tmp/plan.pdf\n\n"));
        let edited = text.replace("Subject: Re: Lunch", "Subject: Re: Lunch, then\n  dinner");
        let back = draft.edited(&edited, None).unwrap();
        assert_eq!(back.subject, "Re: Lunch, then dinner");
        assert_eq!(back.to, draft.to);
        assert_eq!(back.body, draft.body);
        assert_eq!(back.in_reply_to, draft.in_reply_to);
        assert_eq!(back.attachments[0].mime_type, "application/pdf");

        assert!(matches!(
            draft.edited("To: nobody\n\nhi", None),
            Err(Error::Address(_))
        ));
        assert!(matches!(
            draft.edited("Hello there\n\nhi", None),
            Err(Error::Header(1, _))
        ));
    }

    #[test]
    fn built_messages_have_both_bodies_and_their_threads() {
        let mut draft = Draft::reply(
            &original(),
            address("Me", "me@example.com"),
            false,
            "Thursday?",
        );
        draft.body = "**Yes**, see [the menu](https://example.com).\n\n> Thursday?\n".into();
        draft.bcc = vec![address("Boss", "boss@example.com")];
        draft.attachments.push(Attachment {
            name: "notes.txt".into(),
            mime_type: "text/plain".into(),
            source: Source::Bytes(Arc::new(b"notes".to_vec())),
        });
        let built = draft.build("<id@example.com>").unwrap();
        assert_eq!(built.from, "me@example.com");
        assert_eq!(built.recipients, ["ann@example.com", "boss@example.com"]);

        let sent = Message::parse(built.message).unwrap();
        assert_eq!(sent.subject, "Re: Lunch");
        assert_eq!(sent.message_id.as_deref(), Some("id@example.com"));
        assert_eq!(sent.in_reply_to, ["two@example.com"]);
        assert_eq!(sent.references, ["one@example.com", "two@example.com"]);
        assert!(sent.plain().unwrap().0.contains("**Yes**"));
        let html = sent.html().unwrap();
        assert!(
            html.contains("<strong>Yes</strong>") && html.contains("<blockquote>"),
            "{html}"
        );
        assert_eq!(sent.parts()[0].file_name(), "notes.txt");
        assert!(
            !String::from_utf8_lossy(sent.raw()).contains("boss@example.com"),
            "no Bcc sent"
        );
        assert!(
            String::from_utf8_lossy(&built.kept).contains("boss@example.com"),
            "Bcc kept"
        );
    }

    #[test]
    fn new_lines_break_lines() {
        assert!(
            html("-- \nAnn\n").contains("--<br />\nAnn"),
            "{}",
            html("-- \nAnn\n")
        );
    }

    #[test]
    fn postponed_drafts_resume_as_they_were() {
        let mut draft = Draft::reply(
            &original(),
            address("Me", "me@example.com"),
            false,
            "Thursday?",
        );
        draft.body = "Yes.\n\n> Thursday?\n".into();
        draft.bcc = vec![address("Boss", "boss@example.com")];
        draft.attachments.push(Attachment {
            name: "notes.txt".into(),
            mime_type: "text/plain".into(),
            source: Source::Bytes(Arc::new(b"notes".to_vec())),
        });
        let kept = Message::parse(draft.build("<kept@example.com>").unwrap().kept).unwrap();
        let (text, _) = kept.plain().unwrap();
        let resumed = Draft::resumed(&kept, &text);
        assert_eq!(resumed.to, draft.to);
        assert_eq!(resumed.bcc, draft.bcc);
        assert_eq!(resumed.subject, draft.subject);
        assert_eq!(resumed.in_reply_to, draft.in_reply_to);
        assert_eq!(resumed.references, draft.references);
        assert_eq!(resumed.body, draft.body);
        assert_eq!(resumed.attachments, draft.attachments);
    }

    #[test]
    fn markup_typed_in_a_draft_stays_text() {
        assert!(html("<script>alert(1)</script> & co").contains("&lt;script&gt;"));
    }

    #[test]
    fn message_ids_are_at_the_senders_domain() {
        let id = message_id("me@example.com");
        assert!(id.starts_with('<') && id.ends_with("@example.com>"), "{id}");
        assert_ne!(id, message_id("me@example.com"));
    }
}
