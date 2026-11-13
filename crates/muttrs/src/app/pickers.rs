//! The lists muttrs opens over its windows as the kit's pickers (its D14), narrowed as what's typed matches: the
//! Links of the Message shown (to open, or to copy), its Attachments (to open, or to save), the Mailboxes to move
//! to, the Labels, and where a search of every Mailbox found Messages. A picker has one verb, `<CR>`: copying and
//! saving are pickers of their own.

use wcgw_kit::hl::Group;
use wcgw_kit::shell::{Choice, Pick, Picker};

use super::{Kit, Muttrs, Target, Targets};
use crate::hl;
use crate::icons;
use crate::index::truncate;
use crate::sidebar::{self, Row};

/// Makes muttrs' pickers known to `:pick`.
pub(super) fn add(kit: &mut Kit) {
    kit.add_picker("links", |muttrs, kit| {
        muttrs
            .links_picker(kit, false)
            .unwrap_or_else(|why| Picker::list("Links", Vec::new(), why))
    });
    kit.add_picker("yanklinks", |muttrs, kit| {
        muttrs
            .links_picker(kit, true)
            .unwrap_or_else(|why| Picker::list("Copy a Link", Vec::new(), why))
    });
    kit.add_picker("attachments", |muttrs, kit| {
        muttrs.attachments_picker(kit, false)
    });
    kit.add_picker("saveattachments", |muttrs, kit| {
        muttrs.attachments_picker(kit, true)
    });
    kit.add_picker("mailboxes", |muttrs, kit| muttrs.move_picker(kit));
    kit.add_picker("labels", |muttrs, kit| match muttrs.targets(kit) {
        Some(targets) => muttrs.labels_picker(kit, &targets, 0),
        None => Picker::list("Labels", Vec::new(), "No Message here"),
    });
}

impl Muttrs {
    /// The Links of the Message the Pager shows, numbered, to open in the browser, or to copy if `copy`.
    pub(super) fn links_picker(&self, kit: &Kit, copy: bool) -> Result<Picker<Self>, String> {
        let pager = self
            .pagers
            .get(&kit.current_buffer())
            .ok_or_else(|| "Links are the Pager's".to_owned())?;
        let links = pager.links();
        if links.is_empty() {
            return Err("No Links in this Message".to_owned());
        }
        let width = links.len().to_string().len();
        let choices = links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let number = format!("{:>width$} ", index + 1);
                let text = truncate(&link.text, 40);
                let mut label = vec![(number.clone(), hl::ACCENT)];
                if text != link.url {
                    label.push((format!("{text}  "), Group::NORMAL_FLOAT));
                }
                label.push((truncate(&link.url, 80), Group::MARKUP_LINK));
                Choice {
                    label,
                    text: format!("{number}{} {}", link.text, link.url),
                    pick: Pick::Target(if copy {
                        Target::Copy(link.url.clone())
                    } else {
                        Target::Link(link.url.clone())
                    }),
                }
            })
            .collect();
        let title = if copy { "Copy a Link" } else { "Links" };
        Ok(Picker::list(title, choices, "No Link matches"))
    }

    /// The Attachments of the Message the Pager shows, to open, or to save if `save`.
    fn attachments_picker(&self, kit: &Kit, save: bool) -> Picker<Self> {
        let title = if save {
            "Save an Attachment"
        } else {
            "Attachments"
        };
        let current = kit.current_buffer();
        let Some(pager) = self.pagers.get(&current) else {
            return Picker::list(title, Vec::new(), "Attachments are the Pager's");
        };
        let attachments = pager.attachments();
        if attachments.is_empty() {
            return Picker::list(title, Vec::new(), "No Attachments in this Message");
        }
        let icon = kit.icon(icons::ATTACHMENT);
        let choices = attachments
            .iter()
            .enumerate()
            .map(|(part, attachment)| Choice {
                label: vec![
                    (icon.clone(), hl::ATTACHMENT),
                    (attachment.file_name(), Group::NORMAL_FLOAT),
                    (
                        format!(
                            "  {} · {}",
                            attachment.mime_type,
                            crate::pager::size(attachment.size)
                        ),
                        hl::DIM,
                    ),
                ],
                text: attachment.file_name(),
                pick: Pick::Target(Target::Attachment {
                    pager: current,
                    part,
                    save,
                }),
            })
            .collect();
        Picker::list(title, choices, "No Attachment matches")
    }

    /// The Account's other Mailboxes, to move the Messages an action is for to one.
    pub(super) fn move_picker(&self, kit: &Kit) -> Picker<Self> {
        let Some(targets) = self.targets(kit) else {
            return Picker::list("Move to", Vec::new(), "No Message here");
        };
        let title = match targets.uids.len() {
            1 => "Move to".to_owned(),
            count => format!("Move {count} to"),
        };
        let choices: Vec<Choice<Target>> = self
            .moving_to(targets.account, &targets.mailbox)
            .iter()
            .map(|other| {
                let path = other.path.join("/");
                Choice {
                    label: vec![
                        (kit.icon(icons::role(other.role)), hl::DIM),
                        (path.clone(), Group::NORMAL_FLOAT),
                    ],
                    text: path,
                    pick: Pick::Target(Target::Move {
                        targets: targets.clone(),
                        to: other.name.clone(),
                    }),
                }
            })
            .collect();
        Picker::list(title, choices, "No other Mailbox matches")
    }

    /// The Account's Labels, each marked as `targets` have it (`✓` all, `–` some), to add or take off; starting on
    /// the `selecting`th.
    pub(super) fn labels_picker(
        &self,
        kit: &Kit,
        targets: &Targets,
        selecting: usize,
    ) -> Picker<Self> {
        let title = match targets.uids.len() {
            1 => "Labels".to_owned(),
            count => format!("Labels of {count}"),
        };
        let Some(state) = self.accounts.get(targets.account) else {
            return Picker::list(title, Vec::new(), "No Account");
        };
        if state.labels.is_empty() {
            let why = if state.connection == muttrs_mail::client::Connection::Online {
                format!("{} has no Labels", state.name)
            } else {
                format!("{}'s Labels wait for the server", state.name)
            };
            return Picker::list(title, Vec::new(), why);
        }
        let _ = kit;
        let choices = state
            .labels
            .iter()
            .enumerate()
            .map(|(label, named)| {
                let mark = match self.labelled(targets, label) {
                    (0, _) => "  ",
                    (have, all) if have == all => "✓ ",
                    _ => "– ",
                };
                Choice {
                    label: vec![
                        (mark.to_owned(), hl::ACCENT),
                        (named.name.clone(), Group::NORMAL_FLOAT),
                    ],
                    text: named.name.clone(),
                    pick: Pick::Target(Target::Label {
                        targets: targets.clone(),
                        label,
                    }),
                }
            })
            .collect();
        Picker::list(title, choices, "No Label matches").selecting(selecting)
    }

    /// Where a search of every Mailbox of `account` for `query` `found` Messages, how many in each, in the sidebar's
    /// order: choosing one lists what's found there.
    pub(super) fn found_picker(
        &self,
        kit: &Kit,
        account: usize,
        query: &str,
        mut found: Vec<(String, usize)>,
    ) -> Picker<Self> {
        let order = sidebar::rows(&self.accounts);
        found.sort_by_key(|(name, _)| {
            order
                .iter()
                .position(|row| *row == Row::Mailbox(account, name.clone()))
        });
        let total: usize = found.iter().map(|(_, count)| count).sum();
        let state = self.accounts.get(account);
        let choices = found
            .into_iter()
            .map(|(name, count)| {
                let mailbox = state.and_then(|state| state.mailbox(&name));
                let label = mailbox.map_or_else(
                    || muttrs_mail::mailbox::decode(&name),
                    |mailbox| mailbox.path.join("/"),
                );
                Choice {
                    label: vec![
                        (
                            kit.icon(icons::role(mailbox.and_then(|mailbox| mailbox.role))),
                            hl::DIM,
                        ),
                        (label.clone(), Group::NORMAL_FLOAT),
                        (format!("  {count}"), hl::ACCENT),
                    ],
                    text: label,
                    pick: Pick::Target(Target::Found {
                        account,
                        query: query.to_owned(),
                        mailbox: name,
                    }),
                }
            })
            .collect();
        Picker::list(
            format!("{total} found: {query}"),
            choices,
            "No Mailbox matches",
        )
    }
}
