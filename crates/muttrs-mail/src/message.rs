//! A Message read from its bytes: its headers, its bodies, its Inline Images and its Attachments.

use std::fmt;

use jiff::Timestamp;
use mail_parser::{MessageParser, MessagePart, PartKind};

/// An address, as a header names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Address {
    /// The name shown with it, if any.
    pub name: Option<String>,
    /// The address itself, as `ann@example.com`.
    pub email: String,
}

impl Address {
    /// The name if there is one, else the address: what an Index shows of a sender.
    #[must_use]
    pub fn short(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.email)
    }
}

impl fmt::Display for Address {
    /// As a header writes it: `Ann Example <ann@example.com>`, or the address alone.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name.as_deref().filter(|name| !name.trim().is_empty()) {
            Some(name) if name.contains([',', '"', '<', '>', '@', ';', ':']) => {
                write!(f, "\"{}\" <{}>", name.replace('"', "\\\""), self.email)
            }
            Some(name) => write!(f, "{name} <{}>", self.email),
            None => f.write_str(&self.email),
        }
    }
}

/// How a plain text body is written, as its `Content-Type` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextFormat {
    /// Every new line breaks the line.
    #[default]
    Fixed,
    /// RFC 3676's `format=flowed`, its spaces deleted with `delsp=yes`.
    Flowed {
        /// Whether the space ending a flowed line goes.
        delete_space: bool,
    },
}

/// One of a Message's Parts that isn't its body: an Attachment, or an Inline Image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// Its index among the Message's parts.
    id: mail_parser::PartId,
    /// Its MIME type, in lower case: `image/png`.
    pub mime_type: String,
    /// Its file name, if it has one.
    pub name: Option<String>,
    /// Its `Content-ID`, without angle brackets, that an HTML body refers to it by.
    pub content_id: Option<String>,
    /// Its size, decoded, in bytes.
    pub size: usize,
    /// Whether its `Content-Disposition` asks for it to be shown in the body.
    pub inline: bool,
}

impl Part {
    /// A name to save it under: its own, or one made of its type.
    #[must_use]
    pub fn file_name(&self) -> String {
        if let Some(name) = self
            .name
            .as_deref()
            .map(sanitize)
            .filter(|name| !name.is_empty())
        {
            return name;
        }
        let extension =
            self.mime_type
                .split_once('/')
                .map_or("bin", |(_, subtype)| match subtype {
                    "jpeg" => "jpg",
                    "plain" => "txt",
                    "rfc822" => "eml",
                    "octet-stream" => "bin",
                    other => other,
                });
        format!("part-{}.{extension}", self.id)
    }

    /// Whether it's an image.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }
}

/// `name` made safe to save under: no directories, nothing hidden.
fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.trim()
        .trim_start_matches('.')
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// A Message, read.
pub struct Message {
    parsed: mail_parser::Message<'static>,
    /// Who it's from.
    pub from: Vec<Address>,
    /// Who it's to.
    pub to: Vec<Address>,
    /// Who it's copied to.
    pub cc: Vec<Address>,
    /// Who it's copied to unseen: only a Draft kept, or a copy of what was sent, says.
    pub bcc: Vec<Address>,
    /// Where replies go, when not to the sender.
    pub reply_to: Vec<Address>,
    /// Its subject, empty if it has none.
    pub subject: String,
    /// When it was sent, by its `Date`.
    pub date: Option<Timestamp>,
    /// Its `Message-ID`, without angle brackets.
    pub message_id: Option<String>,
    /// The `Message-ID`s it replies to, without angle brackets.
    pub in_reply_to: Vec<String>,
    /// The `Message-ID`s of the Messages before it in its thread, oldest first.
    pub references: Vec<String>,
    /// The mailing list it came through, by name: its `List-Id`'s description, else its id.
    pub list: Option<String>,
    /// The address of the mailing list it came through, to write to it (its `List-Post`, RFC 2369): None if it
    /// came through none, or one that takes no posts (`List-Post: NO`).
    pub list_post: Option<String>,
    /// How to unsubscribe from the mailing list it came through (its `List-Unsubscribe`): URLs, `mailto:` or
    /// `https:`, in the order given, the list's preferred first.
    pub list_unsubscribe: Vec<String>,
    /// Whether its `https:` URL to unsubscribe does it with a POST alone, without a page to confirm it on
    /// (`List-Unsubscribe-Post: List-Unsubscribe=One-Click`, RFC 8058).
    pub one_click_unsubscribe: bool,
    html: Option<mail_parser::PartId>,
    plain: Option<(mail_parser::PartId, TextFormat)>,
    parts: Vec<Part>,
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message")
            .field("from", &self.from)
            .field("subject", &self.subject)
            .field("date", &self.date)
            .field("parts", &self.parts)
            .finish_non_exhaustive()
    }
}

impl Message {
    /// Reads a Message from its bytes, as RFC 5322 writes it. Damaged structure is read as well as it can be.
    ///
    /// # Errors
    ///
    /// If `bytes` is no message at all, as an empty file isn't.
    pub fn parse(bytes: Vec<u8>) -> Result<Self, Unreadable> {
        let parsed = MessageParser::new().parse_owned(bytes).ok_or(Unreadable)?;
        let root = parsed.root();
        let html = root
            .html_body()
            .find(|part| matches!(part.kind(), PartKind::Html));
        let plain = root
            .text_body()
            .find(|part| matches!(part.kind(), PartKind::Text));
        let body_ids: Vec<mail_parser::PartId> = html
            .iter()
            .chain(plain.iter())
            .map(MessagePart::id)
            .collect();
        let parts = root
            .attachments()
            .filter(|part| !body_ids.contains(&part.id()))
            .map(|part| describe(&part))
            .collect();
        Ok(Self {
            from: addresses(root.from()),
            to: addresses(root.to()),
            cc: addresses(root.cc()),
            bcc: addresses(root.bcc()),
            reply_to: addresses(root.reply_to()),
            subject: root.subject().unwrap_or_default().trim().to_owned(),
            date: timestamp(root.date()),
            message_id: root.message_id().map(|id| strip_brackets(id).to_owned()),
            in_reply_to: ids(root.in_reply_to()),
            references: ids(root.references()),
            list: root.list_id().and_then(|list| {
                let list = list.mailboxes().next()?;
                list.name()
                    .filter(|name| !name.trim().is_empty())
                    .or(list.address())
                    .map(|name| strip_brackets(name).to_owned())
            }),
            list_post: urls(root.list_post())
                .iter()
                .find_map(|url| crate::compose::mailto(url))
                .map(|mailto| mailto.to),
            list_unsubscribe: urls(root.list_unsubscribe()),
            one_click_unsubscribe: root
                .headers()
                .get("List-Unsubscribe-Post")
                .and_then(|header| header.value().as_text())
                .is_some_and(|value| {
                    value
                        .trim()
                        .eq_ignore_ascii_case("List-Unsubscribe=One-Click")
                }),
            html: html.map(|part| part.id()),
            plain: plain.map(|part| (part.id(), text_format(&part))),
            parts,
            parsed,
        })
    }

    /// How to unsubscribe from the mailing list it came through, as its `List-Unsubscribe` says, in the order RFC
    /// 8058 prefers: by a POST alone where it says one does; else by mail; else at a page. None if it says no way.
    #[must_use]
    pub fn unsubscribe(&self) -> Option<Unsubscribe> {
        let page = self.list_unsubscribe.iter().find(|url| {
            url.get(..8)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        });
        if self.one_click_unsubscribe
            && let Some(url) = page
        {
            return Some(Unsubscribe::OneClick(url.clone()));
        }
        self.list_unsubscribe
            .iter()
            .find_map(|url| crate::compose::mailto(url))
            .map(Unsubscribe::Mail)
            .or_else(|| page.map(|url| Unsubscribe::Page(url.clone())))
    }

    /// Its headers, every one, in order, as written: each's name, and its value unfolded onto one line (encoded
    /// words left as they are).
    #[must_use]
    pub fn headers(&self) -> Vec<(String, String)> {
        self.parsed
            .root()
            .headers()
            .iter()
            .map(|header| {
                let value = String::from_utf8_lossy(header.raw_value());
                let unfolded = value
                    .split(['\r', '\n'])
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                (header.raw_name().to_owned(), unfolded)
            })
            .collect()
    }

    /// The whole of it, as it came: its headers and its body, as RFC 5322 writes them (what isn't UTF-8 shown as
    /// best it can be).
    #[must_use]
    pub fn source(&self) -> String {
        String::from_utf8_lossy(self.raw()).into_owned()
    }

    /// Its HTML body, if it has one.
    #[must_use]
    pub fn html(&self) -> Option<String> {
        let part = self.parsed.part(self.html?)?;
        part.html_utf8()
            .or_else(|| part.text())
            .map(std::borrow::Cow::into_owned)
    }

    /// Its plain text body, if it has one, and how it's written.
    #[must_use]
    pub fn plain(&self) -> Option<(String, TextFormat)> {
        let (id, format) = self.plain?;
        let text = self.parsed.part(id)?.text()?;
        Some((text.into_owned(), format))
    }

    /// Its Parts besides its bodies: Attachments and Inline Images.
    #[must_use]
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// The Part an HTML body refers to as `cid:` `content_id`.
    #[must_use]
    pub fn by_content_id(&self, content_id: &str) -> Option<&Part> {
        self.parts.iter().find(|part| {
            part.content_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(content_id))
        })
    }

    /// The decoded bytes of `part`.
    #[must_use]
    pub fn bytes(&self, part: &Part) -> Vec<u8> {
        self.parsed
            .part(part.id)
            .map(|part| part.decoded().into_owned())
            .unwrap_or_default()
    }

    /// The bytes it was read from.
    #[must_use]
    pub fn raw(&self) -> &[u8] {
        self.parsed.raw()
    }

    /// Whether it has Attachments: Parts that aren't Inline Images.
    #[must_use]
    pub fn has_attachments(&self) -> bool {
        self.parts
            .iter()
            .any(|part| !part.inline || part.content_id.is_none())
    }
}

fn describe(part: &MessagePart<'_>) -> Part {
    let mime_type = part.content_type().map_or_else(
        || "application/octet-stream".to_owned(),
        |kind| match kind.subtype() {
            Some(subtype) => format!("{}/{subtype}", kind.ctype()).to_ascii_lowercase(),
            None => kind.ctype().to_ascii_lowercase(),
        },
    );
    let name = part.attachment_name().map(str::to_owned).or_else(|| {
        part.nested()
            .and_then(|message| message.subject().map(|subject| format!("{subject}.eml")))
    });
    Part {
        id: part.id(),
        mime_type,
        name,
        content_id: part.content_id().map(|id| strip_brackets(id).to_owned()),
        size: part.decoded_len(),
        inline: part.is_inline(),
    }
}

fn text_format(part: &MessagePart<'_>) -> TextFormat {
    let Some(kind) = part.content_type() else {
        return TextFormat::Fixed;
    };
    if kind
        .attribute("format")
        .is_some_and(|format| format.eq_ignore_ascii_case("flowed"))
    {
        TextFormat::Flowed {
            delete_space: kind
                .attribute("delsp")
                .is_some_and(|delsp| delsp.eq_ignore_ascii_case("yes")),
        }
    } else {
        TextFormat::Fixed
    }
}

/// The addresses of a header.
pub(crate) fn addresses(list: Option<mail_parser::AddressList<'_>>) -> Vec<Address> {
    list.map(|list| {
        list.mailboxes()
            .filter_map(|mailbox| {
                Some(Address {
                    name: mailbox.name().map(str::to_owned),
                    email: mailbox.address()?.to_owned(),
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// How to unsubscribe from a mailing list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unsubscribe {
    /// By a POST to this URL, with nothing to confirm (RFC 8058).
    OneClick(String),
    /// By mail.
    Mail(crate::compose::Mailto),
    /// At the page at this URL, in a browser.
    Page(String),
}

/// The URLs of a `List-` header (RFC 2369), in order.
fn urls(list: Option<mail_parser::AddressList<'_>>) -> Vec<String> {
    list.map(|list| {
        list.mailboxes()
            .filter_map(|mailbox| mailbox.address())
            .map(|url| strip_brackets(url).to_owned())
            .filter(|url| !url.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// The `Message-ID`s of a header, without angle brackets.
pub(crate) fn ids(list: Option<mail_parser::TextList<'_>>) -> Vec<String> {
    list.map(|list| {
        list.iter()
            .map(|id| strip_brackets(id).to_owned())
            .collect()
    })
    .unwrap_or_default()
}

/// A `Date` header as a timestamp, if it's a valid date.
pub(crate) fn timestamp(date: Option<mail_parser::DateTime>) -> Option<Timestamp> {
    date.filter(mail_parser::DateTime::is_valid)
        .and_then(|date| Timestamp::from_second(date.to_timestamp()).ok())
}

pub(crate) fn strip_brackets(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

/// The bytes given aren't a Message.
#[derive(Debug, thiserror::Error)]
#[error("this isn't an email message")]
pub struct Unreadable;

#[cfg(test)]
mod tests {
    use super::*;

    const RELATED: &str = "From: \"Example, Ann\" <ann@example.com>\r
To: bob@example.com, Carol <carol@example.com>\r
Subject: =?UTF-8?Q?Caf=C3=A9?= plans\r
Date: Mon, 2 Nov 2026 09:30:00 -0500\r
Message-ID: <one@example.com>\r
In-Reply-To: <zero@example.com>\r
References: <minus@example.com> <zero@example.com>\r
MIME-Version: 1.0\r
Content-Type: multipart/mixed; boundary=outer\r
\r
--outer\r
Content-Type: multipart/related; boundary=related\r
\r
--related\r
Content-Type: multipart/alternative; boundary=alt\r
\r
--alt\r
Content-Type: text/plain; charset=utf-8; format=flowed; delsp=yes\r
\r
Hello \r
there\r
--alt\r
Content-Type: text/html; charset=iso-8859-1\r
Content-Transfer-Encoding: quoted-printable\r
\r
<p>Caf=E9 <img src=3D\"cid:logo@x\"></p>\r
--alt--\r
--related\r
Content-Type: image/png\r
Content-ID: <logo@x>\r
Content-Disposition: inline\r
Content-Transfer-Encoding: base64\r
\r
iVBORw0KGgo=\r
--related--\r
--outer\r
Content-Type: application/pdf; name=\"../../plan.pdf\"\r
Content-Disposition: attachment; filename=\"../../plan.pdf\"\r
Content-Transfer-Encoding: base64\r
\r
JVBERi0=\r
--outer--\r
";

    #[test]
    fn headers_bodies_and_parts() {
        let message = Message::parse(RELATED.as_bytes().to_vec()).unwrap();
        assert_eq!(
            message.from[0].to_string(),
            "\"Example, Ann\" <ann@example.com>"
        );
        assert_eq!(message.to[1].short(), "Carol");
        assert_eq!(message.to[0].short(), "bob@example.com");
        assert_eq!(message.subject, "Café plans");
        assert_eq!(
            message.date.unwrap().to_string(),
            "2026-11-02T14:30:00Z",
            "in UTC"
        );
        assert_eq!(message.message_id.as_deref(), Some("one@example.com"));
        assert_eq!(message.in_reply_to, ["zero@example.com"]);
        assert_eq!(
            message.references,
            ["minus@example.com", "zero@example.com"]
        );

        assert_eq!(
            message.html().unwrap().trim(),
            "<p>Café <img src=\"cid:logo@x\"></p>"
        );
        let (plain, format) = message.plain().unwrap();
        assert_eq!(plain, "Hello \r\nthere");
        assert_eq!(format, TextFormat::Flowed { delete_space: true });

        let logo = message.by_content_id("logo@x").expect("the Inline Image");
        assert!(logo.inline && logo.is_image());
        assert_eq!(message.bytes(logo), b"\x89PNG\r\n\x1a\n");

        let pdf = message
            .parts()
            .iter()
            .find(|part| part.mime_type == "application/pdf")
            .unwrap();
        assert_eq!(
            pdf.file_name(),
            "plan.pdf",
            "no way out of the directory it's saved in"
        );
        assert!(message.has_attachments());
    }

    #[test]
    fn a_plain_message() {
        let message = Message::parse(b"Subject: hi\r\n\r\njust text\r\n".to_vec()).unwrap();
        assert_eq!(message.html(), None);
        assert_eq!(message.plain().unwrap().0, "just text\r\n");
        assert_eq!(message.parts(), []);
        assert!(!message.has_attachments());
    }

    #[test]
    fn a_mailing_lists_headers_say_where_to_post_and_how_to_unsubscribe() {
        let message = |headers: &str| {
            Message::parse(
                format!("From: ann@example.com\r\n{headers}Subject: s\r\n\r\nbody\r\n")
                    .into_bytes(),
            )
            .unwrap()
        };
        let listed = message(
            "List-Id: Rust for Linux <rust-for-linux.vger.kernel.org>\r\n\
             List-Post: <mailto:rust-for-linux@vger.kernel.org>\r\n\
             List-Unsubscribe: <mailto:rust-for-linux+unsubscribe@vger.kernel.org?subject=leave%20now>,\r\n \
             <https://lists.example.org/u/abc>\r\n\
             List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        );
        assert_eq!(listed.list.as_deref(), Some("Rust for Linux"));
        assert_eq!(
            listed.list_post.as_deref(),
            Some("rust-for-linux@vger.kernel.org")
        );
        assert_eq!(listed.list_unsubscribe.len(), 2);
        assert_eq!(
            listed.unsubscribe(),
            Some(Unsubscribe::OneClick(
                "https://lists.example.org/u/abc".into()
            )),
            "one click, where it says it takes one"
        );
        let by_mail = message(
            "List-Unsubscribe: <https://lists.example.org/u/abc>, \
             <mailto:leave@lists.example.org?subject=leave%20now>\r\n",
        );
        assert_eq!(
            by_mail.unsubscribe(),
            Some(Unsubscribe::Mail(crate::compose::Mailto {
                to: "leave@lists.example.org".into(),
                subject: Some("leave now".into()),
                body: None,
            })),
            "by mail before a page, which wants confirming"
        );
        let at_a_page =
            message("List-Unsubscribe: <https://lists.example.org/u/abc>\r\nList-Post: NO\r\n");
        assert_eq!(
            at_a_page.unsubscribe(),
            Some(Unsubscribe::Page("https://lists.example.org/u/abc".into()))
        );
        assert_eq!(at_a_page.list_post, None, "a list that takes no posts");
        assert_eq!(message("").unsubscribe(), None);
        assert_eq!(message("").list, None);
    }

    #[test]
    fn every_header_shows_as_written_and_unfolded_and_the_source_as_it_came() {
        let bytes = b"Received: from a.example\r\n\tby b.example; Mon, 2 Nov 2026\r\n\
                      Subject: =?utf-8?q?caf=C3=A9?=\r\nX-Spam: no\r\n\r\nbody\r\n";
        let message = Message::parse(bytes.to_vec()).unwrap();
        assert_eq!(
            message.headers(),
            [
                (
                    "Received".to_owned(),
                    "from a.example by b.example; Mon, 2 Nov 2026".to_owned()
                ),
                ("Subject".to_owned(), "=?utf-8?q?caf=C3=A9?=".to_owned()),
                ("X-Spam".to_owned(), "no".to_owned()),
            ]
        );
        assert_eq!(message.subject, "café", "as read");
        assert_eq!(message.source().as_bytes(), bytes);
    }
}
