//! Mailboxes: their names (IMAP's modified UTF-7 on the wire), their Roles, their counts.

use base64::Engine as _;
use base64::alphabet::Alphabet;
use base64::engine::GeneralPurpose;
use base64::engine::general_purpose::NO_PAD;

/// What a Mailbox is for, when it has a special use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// Where new mail arrives.
    Inbox,
    /// Where Drafts are Postponed, to finish later.
    Drafts,
    /// Copies of what was sent.
    Sent,
    /// Messages archived out of the Inbox.
    Archive,
    /// Every Message of the Account, as Gmail's All Mail.
    All,
    /// Every Flagged Message, as Gmail's Starred.
    Flagged,
    /// Messages taken for spam.
    Junk,
    /// Deleted Messages, until they're expunged from it.
    Trash,
}

/// A Mailbox on an Account's server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mailbox {
    /// Its name as the server knows it, in modified UTF-7: what commands name it by.
    pub name: String,
    /// Its name for people, decoded, and its path split on the hierarchy's delimiter.
    pub path: Vec<String>,
    /// Its Role, as its name (INBOX) or the server (SPECIAL-USE) says.
    pub role: Option<Role>,
    /// Whether it can be opened; `[Gmail]` can't, it only holds others.
    pub selectable: bool,
}

impl Mailbox {
    /// A Mailbox the server listed, as `name` with these attributes, its hierarchy delimited by `delimiter`.
    #[must_use]
    pub fn listed(name: &str, delimiter: Option<&str>, attributes: &[&str]) -> Self {
        let decoded = decode(name);
        let path = match delimiter.filter(|delimiter| !delimiter.is_empty()) {
            Some(delimiter) => decoded.split(delimiter).map(str::to_owned).collect(),
            None => vec![decoded],
        };
        let has = |attribute: &str| attributes.iter().any(|a| a.eq_ignore_ascii_case(attribute));
        let role = if name.eq_ignore_ascii_case("INBOX") {
            Some(Role::Inbox)
        } else {
            [
                ("\\Sent", Role::Sent),
                ("\\Drafts", Role::Drafts),
                ("\\Trash", Role::Trash),
                ("\\Junk", Role::Junk),
                ("\\Archive", Role::Archive),
                ("\\All", Role::All),
                ("\\Flagged", Role::Flagged),
            ]
            .into_iter()
            .find_map(|(attribute, role)| has(attribute).then_some(role))
        };
        Self {
            name: name.to_owned(),
            path,
            role,
            selectable: !has("\\Noselect") && !has("\\NonExistent"),
        }
    }

    /// Its name for people: its path's last part, or "Inbox".
    #[must_use]
    pub fn label(&self) -> &str {
        if self.role == Some(Role::Inbox) {
            return "Inbox";
        }
        self.path.last().map_or(&self.name, String::as_str)
    }

    /// How deep in the hierarchy it is: 0 at the top.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.path.len().saturating_sub(1)
    }
}

/// How many Messages a Mailbox has, and how many are Unread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// How many Messages it has.
    pub messages: u32,
    /// How many of them are Unread.
    pub unread: u32,
}

/// IMAP's base64: standard, with `,` for `/`, and no padding.
const IMAP_BASE64: GeneralPurpose = GeneralPurpose::new(
    &match Alphabet::new("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,") {
        Ok(alphabet) => alphabet,
        Err(_) => panic!("a valid alphabet"),
    },
    NO_PAD,
);

/// A Mailbox name from modified UTF-7 (RFC 3501 §5.1.3), as servers send it. What isn't valid is kept as it is.
#[must_use]
pub fn decode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('-') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let encoded = &after[..end];
        if encoded.is_empty() {
            out.push('&');
        } else {
            let decoded = IMAP_BASE64.decode(encoded).ok().and_then(|bytes| {
                let units: Vec<u16> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect();
                String::from_utf16(&units).ok()
            });
            match decoded {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[start..=start + end + 1]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A Mailbox name in modified UTF-7, as servers take it.
#[must_use]
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut pending: Vec<u16> = Vec::new();
    let flush = |pending: &mut Vec<u16>, out: &mut String| {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|unit| unit.to_be_bytes()).collect();
        out.push('&');
        out.push_str(&IMAP_BASE64.encode(bytes));
        out.push('-');
        pending.clear();
    };
    for character in name.chars() {
        if (' '..='~').contains(&character) {
            flush(&mut pending, &mut out);
            if character == '&' {
                out.push_str("&-");
            } else {
                out.push(character);
            }
        } else {
            let mut units = [0; 2];
            pending.extend_from_slice(character.encode_utf16(&mut units));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_utf7_round_trips() {
        for (wire, name) in [
            ("INBOX", "INBOX"),
            ("Entw&APw-rfe", "Entwürfe"),
            ("&ZeVnLIqe-", "日本語"),
            ("Tom &- Jerry", "Tom & Jerry"),
            ("&2D3eAA-", "😀"),
        ] {
            assert_eq!(decode(wire), name);
            assert_eq!(encode(name), wire);
        }
        assert_eq!(decode("broken &###-"), "broken &###-", "kept as it is");
    }

    #[test]
    fn roles_from_special_use() {
        let sent = Mailbox::listed(
            "[Gmail]/Sent Mail",
            Some("/"),
            &["\\HasNoChildren", "\\Sent"],
        );
        assert_eq!(sent.role, Some(Role::Sent));
        assert_eq!(sent.path, ["[Gmail]", "Sent Mail"]);
        assert_eq!(sent.label(), "Sent Mail");
        assert_eq!(sent.depth(), 1);
        let gmail = Mailbox::listed("[Gmail]", Some("/"), &["\\Noselect", "\\HasChildren"]);
        assert!(!gmail.selectable);
        assert_eq!(
            Mailbox::listed("inbox", Some("/"), &[]).role,
            Some(Role::Inbox)
        );
    }
}
