//! Envelopes: what the Index shows of a Message without fetching it.

use jiff::Timestamp;
use mail_parser::MessageParser;

use crate::message::{self, Address};

/// The header fields an Envelope is made of, as `BODY.PEEK[HEADER.FIELDS (…)]` asks for them.
pub(crate) const FIELDS: &str =
    "FROM TO CC SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES CONTENT-TYPE LIST-ID";

/// The Flags a Message carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
// One bool a Flag, as IMAP has them: each is on or off whatever the others are, so they aren't a state machine.
#[allow(clippy::struct_excessive_bools)]
pub struct Flags {
    /// It's been read; without Seen, it's Unread.
    pub seen: bool,
    /// The user marked it as important; Gmail calls it starred.
    pub flagged: bool,
    /// It's been replied to.
    pub answered: bool,
    /// It's a Draft, not yet sent.
    pub draft: bool,
    /// It's marked to be expunged.
    pub deleted: bool,
}

impl Flags {
    /// The Flags named, as IMAP writes them: `\Seen`, `\Flagged`, …; others are ignored.
    #[must_use]
    pub fn named<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut flags = Self::default();
        for name in names {
            match name.to_ascii_lowercase().as_str() {
                "\\seen" => flags.seen = true,
                "\\flagged" => flags.flagged = true,
                "\\answered" => flags.answered = true,
                "\\draft" => flags.draft = true,
                "\\deleted" => flags.deleted = true,
                _ => {}
            }
        }
        flags
    }
}

/// What the Index shows of a Message.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
    /// Its UID in its Mailbox.
    pub uid: u32,
    /// The Flags it carries.
    pub flags: Flags,
    /// When it was sent, by its `Date`, or else when the server got it.
    pub date: Option<Timestamp>,
    /// Who it's from.
    pub from: Vec<Address>,
    /// Who it's to.
    pub to: Vec<Address>,
    /// Who it's copied to.
    pub cc: Vec<Address>,
    /// Its subject, empty if it has none.
    pub subject: String,
    /// Its `Message-ID`, without angle brackets.
    pub message_id: Option<String>,
    /// The `Message-ID`s it replies to, without angle brackets.
    pub in_reply_to: Vec<String>,
    /// The `Message-ID`s of the Messages before it in its thread, oldest first.
    pub references: Vec<String>,
    /// Its size, in bytes.
    pub size: u32,
    /// Whether it likely has Attachments: its body is `multipart/mixed`.
    pub has_attachments: bool,
}

impl Envelope {
    /// The Envelope of Message `uid`, from its header fields as fetched, its Flags, the time the server got it,
    /// and its size.
    #[must_use]
    pub fn read(
        uid: u32,
        flags: Flags,
        received: Option<Timestamp>,
        size: u32,
        headers: &[u8],
    ) -> Self {
        let Some(parsed) = MessageParser::new().parse_headers(headers) else {
            return Self {
                uid,
                flags,
                date: received,
                from: Vec::new(),
                to: Vec::new(),
                cc: Vec::new(),
                subject: String::new(),
                message_id: None,
                in_reply_to: Vec::new(),
                references: Vec::new(),
                size,
                has_attachments: false,
            };
        };
        let root = parsed.root();
        let has_attachments = root.content_type().is_some_and(|kind| {
            kind.ctype().eq_ignore_ascii_case("multipart")
                && kind
                    .subtype()
                    .is_some_and(|subtype| subtype.eq_ignore_ascii_case("mixed"))
        });
        Self {
            uid,
            flags,
            date: message::timestamp(root.date()).or(received),
            from: message::addresses(root.from()),
            to: message::addresses(root.to()),
            cc: message::addresses(root.cc()),
            subject: root.subject().unwrap_or_default().trim().to_owned(),
            message_id: root
                .message_id()
                .map(|id| message::strip_brackets(id).to_owned()),
            in_reply_to: message::ids(root.in_reply_to()),
            references: message::ids(root.references()),
            size,
            has_attachments,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_from_header_fields() {
        let envelope = Envelope::read(
            7,
            Flags::named(["\\Seen", "\\Flagged", "$Forwarded"]),
            None,
            1234,
            b"From: =?utf-8?B?SsO8cmdlbg==?= <j@example.com>\r\nSubject: =?iso-8859-1?q?R=E9union?=\r\n\
              Date: Tue, 3 Nov 2026 08:00:00 +0000\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n",
        );
        assert_eq!(envelope.from[0].short(), "Jürgen");
        assert_eq!(envelope.subject, "Réunion");
        assert_eq!(envelope.date.unwrap().to_string(), "2026-11-03T08:00:00Z");
        assert!(envelope.flags.seen && envelope.flags.flagged && !envelope.flags.answered);
        assert!(envelope.has_attachments);
    }

    #[test]
    fn without_a_date_the_server_says_when() {
        let received = Timestamp::from_second(1_800_000_000).unwrap();
        let envelope = Envelope::read(
            1,
            Flags::default(),
            Some(received),
            0,
            b"Subject: hi\r\n\r\n",
        );
        assert_eq!(envelope.date, Some(received));
    }
}
