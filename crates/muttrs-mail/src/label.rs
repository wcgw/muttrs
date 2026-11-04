//! Labels: Mailboxes a Message can be in as well as the one it's listed in (see `CONTEXT.md`). On Gmail, its own
//! labels, which it tells of each Message (`X-GM-LABELS`); elsewhere, as on Proton Mail's Bridge, the Mailboxes under
//! `Labels`, a Message labelled by a copy of it there, found by its `Message-ID` (see `docs/adr/0005-labels.md`).

use std::collections::{BTreeMap, HashSet};

use crate::envelope::Envelope;
use crate::imap::Selected;
use crate::mailbox::{self, Mailbox};

/// The Mailbox Proton Mail's Bridge keeps its Labels under, each a Mailbox of its own.
const LABELS: &str = "Labels";

/// A Label of an Account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    /// Its name for people: `GH`, of Proton's `Labels/GH`; `Work/Reports`, of Gmail's.
    pub name: String,
    /// The Mailbox that has its Messages, by its name on the server.
    pub mailbox: String,
}

/// The Labels among `mailboxes`, by name: on Gmail (`gmail`), those that aren't its own (`[Gmail]/…`, the Inbox),
/// elsewhere those under `Labels`.
#[must_use]
pub fn of(mailboxes: &[Mailbox], gmail: bool) -> Vec<Label> {
    let mut labels: Vec<Label> = mailboxes
        .iter()
        .filter(|mailbox| mailbox.selectable)
        .filter_map(|mailbox| {
            let name = if gmail {
                let own = mailbox.role.is_some()
                    || mailbox
                        .path
                        .first()
                        .is_some_and(|top| top.starts_with('[') && top.ends_with(']'));
                (!own).then(|| mailbox.path.join("/"))?
            } else {
                match mailbox.path.as_slice() {
                    [top, under @ ..]
                        if top == LABELS && !under.is_empty() && mailbox.role.is_none() =>
                    {
                        under.join("/")
                    }
                    _ => return None,
                }
            };
            Some(Label {
                name,
                mailbox: mailbox.name.clone(),
            })
        })
        .collect();
    labels.sort_by(|a, b| order(&a.name, &b.name));
    labels
}

/// The order Labels' names are listed in: by name, ignoring case.
#[must_use]
pub fn order(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// The name for people of a label as Gmail's `X-GM-LABELS` names it; None for its own (`\Inbox`, `\Starred`).
pub(crate) fn gmail_name(label: &str) -> Option<String> {
    (!label.starts_with('\\')).then(|| mailbox::decode(label))
}

/// What's known of a Label's Mailbox where Labels are Mailboxes of their own: which Messages it has, by
/// `Message-ID`, and how it stood when they were listed, to tell whether it changed since.
#[derive(Clone, Debug, Default)]
pub(crate) struct Members {
    /// How it stood when its Messages were listed, as STATUS tells: how many, its UIDVALIDITY and UIDNEXT; None
    /// before.
    pub(crate) listed: Option<Selected>,
    /// Its Messages' `Message-ID`s, by UID; None for one without.
    ids: BTreeMap<u32, Option<String>>,
}

impl Members {
    /// Whether what's listed is still what the Mailbox has, as it stands now.
    pub(crate) fn is_current(&self, now: Selected) -> bool {
        self.listed == Some(now) && now.uid_next.is_some()
    }

    /// Keeps the Messages of `uids`, all the Mailbox has now (its UIDVALIDITY `uid_validity`), and forgets those
    /// it no longer has: the UIDs whose `Message-ID` is to be listed are returned.
    pub(crate) fn keep(&mut self, uid_validity: u32, uids: &[u32]) -> Vec<u32> {
        if self
            .listed
            .is_some_and(|listed| listed.uid_validity != uid_validity)
        {
            self.ids.clear();
        }
        let now: HashSet<u32> = uids.iter().copied().collect();
        self.ids.retain(|uid, _| now.contains(uid));
        let mut lacking: Vec<u32> = uids
            .iter()
            .copied()
            .filter(|uid| !self.ids.contains_key(uid))
            .collect();
        lacking.sort_unstable();
        lacking
    }

    /// Lists Message `uid`'s `Message-ID`.
    pub(crate) fn add(&mut self, uid: u32, message_id: Option<String>) {
        self.ids.insert(uid, message_id);
    }

    /// The `Message-ID`s it has.
    pub(crate) fn message_ids(&self) -> HashSet<&str> {
        self.ids.values().flatten().map(String::as_str).collect()
    }

    /// The UIDs of its Messages with any of `message_ids`.
    pub(crate) fn uids_of(&self, message_ids: &HashSet<&str>) -> Vec<u32> {
        self.ids
            .iter()
            .filter(|(_, id)| id.as_deref().is_some_and(|id| message_ids.contains(id)))
            .map(|(&uid, _)| uid)
            .collect()
    }

    /// Forgets Messages `uids`, gone from it.
    pub(crate) fn forget(&mut self, uids: &[u32]) {
        for uid in uids {
            self.ids.remove(uid);
        }
        self.listed = None;
    }
}

/// Gives each of `envelopes` the names of `labels` whose Members have its `Message-ID`, in `labels`' order.
pub(crate) fn label<'a>(
    envelopes: impl IntoIterator<Item = &'a mut Envelope>,
    labels: &[(&Label, &Members)],
) {
    let ids: Vec<(&str, HashSet<&str>)> = labels
        .iter()
        .map(|(label, members)| (label.name.as_str(), members.message_ids()))
        .collect();
    for envelope in envelopes {
        envelope.labels = envelope.message_id.as_deref().map_or_else(Vec::new, |id| {
            ids.iter()
                .filter(|(_, members)| members.contains(id))
                .map(|(name, _)| (*name).to_owned())
                .collect()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Flags;

    fn listed(name: &str, attributes: &[&str]) -> Mailbox {
        Mailbox::listed(name, Some("/"), attributes)
    }

    fn names(labels: &[Label]) -> Vec<(&str, &str)> {
        labels
            .iter()
            .map(|label| (label.name.as_str(), label.mailbox.as_str()))
            .collect()
    }

    #[test]
    fn protons_labels_are_under_labels() {
        let mailboxes = [
            listed("INBOX", &[]),
            listed("Folders", &["\\Noselect"]),
            listed("Folders/RfL", &[]),
            listed("Labels", &["\\Noselect"]),
            listed("Labels/Rust for Linux", &[]),
            listed("Labels/GH", &[]),
            listed("Starred", &["\\Flagged"]),
        ];
        assert_eq!(
            names(&of(&mailboxes, false)),
            [
                ("GH", "Labels/GH"),
                ("Rust for Linux", "Labels/Rust for Linux")
            ]
        );
    }

    #[test]
    fn gmails_labels_are_its_mailboxes_but_its_own() {
        let mailboxes = [
            listed("INBOX", &[]),
            listed("[Gmail]", &["\\Noselect"]),
            listed("[Gmail]/All Mail", &["\\All"]),
            listed("[Gmail]/Important", &[]),
            listed("Work", &[]),
            listed("Work/Reports", &[]),
            listed("&AMk-t&AOk-", &[]),
        ];
        assert_eq!(
            names(&of(&mailboxes, true)),
            [
                ("Work", "Work"),
                ("Work/Reports", "Work/Reports"),
                ("Été", "&AMk-t&AOk-")
            ]
        );
        assert!(of(&mailboxes, false).is_empty(), "no Labels elsewhere");
    }

    #[test]
    fn gmails_own_labels_are_left_out() {
        assert_eq!(gmail_name("\\Inbox"), None);
        assert_eq!(gmail_name("Work").as_deref(), Some("Work"));
        assert_eq!(gmail_name("&AMk-t&AOk-").as_deref(), Some("Été"));
    }

    #[test]
    fn members_list_what_changed_and_find_messages_by_their_ids() {
        let mut members = Members::default();
        let standing = Selected {
            exists: 3,
            uid_validity: 7,
            uid_next: Some(4),
            highest_modseq: None,
        };
        assert!(!members.is_current(standing));
        assert_eq!(members.keep(7, &[1, 2, 3]), [1, 2, 3]);
        members.add(1, Some("a@x".into()));
        members.add(2, None);
        members.add(3, Some("c@x".into()));
        members.listed = Some(standing);
        assert!(members.is_current(standing));

        assert_eq!(members.keep(7, &[1, 3, 5]), [5], "2 gone, 5 new");
        members.add(5, Some("e@x".into()));
        assert_eq!(
            members.uids_of(&["a@x", "e@x", "z@x"].into_iter().collect()),
            [1, 5]
        );
        members.forget(&[1]);
        assert_eq!(members.message_ids(), ["c@x", "e@x"].into_iter().collect());
        assert_eq!(members.keep(8, &[1]), [1], "renumbered: listed again");
    }

    #[test]
    fn envelopes_get_the_labels_whose_mailboxes_have_their_ids() {
        let named = |name: &str| Label {
            name: name.to_owned(),
            mailbox: format!("Labels/{name}"),
        };
        let (gh, rfl) = (named("GH"), named("RfL"));
        let mut has_a = Members::default();
        has_a.keep(1, &[1]);
        has_a.add(1, Some("a@x".into()));
        let mut has_both = has_a.clone();
        has_both.keep(1, &[1, 2]);
        has_both.add(2, Some("b@x".into()));
        let envelope = |id: Option<&str>| Envelope {
            message_id: id.map(str::to_owned),
            labels: vec!["stale".into()],
            ..Envelope::read(1, Flags::default(), None, 0, b"")
        };
        let mut envelopes = [envelope(Some("a@x")), envelope(Some("b@x")), envelope(None)];
        label(&mut envelopes, &[(&gh, &has_both), (&rfl, &has_a)]);
        let labels: Vec<&[String]> = envelopes.iter().map(|e| e.labels.as_slice()).collect();
        assert_eq!(
            labels,
            [
                &["GH".to_owned(), "RfL".to_owned()][..],
                &["GH".to_owned()],
                &[]
            ]
        );
    }
}
