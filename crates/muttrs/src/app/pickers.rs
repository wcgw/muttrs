//! The lists muttrs opens over its windows as the kit's pickers (its D14), narrowed as what's typed matches: the
//! Links of the Message shown (`<CR>` opens one, `<C-y>` yanks it), its Attachments (`<CR>` opens one, `<C-s>`
//! saves it), the Mailboxes to open (`<leader>ff`) or to move to (`<leader>m`), the Labels, and where a search of
//! every Mailbox found Messages.

use wcgw_kit::hl::Group;
use wcgw_kit::shell::{Choice, Pick, Picker};

use super::{Effect, Effects, Kit, Muttrs, Target, Targets};
use crate::hl;
use crate::icons;
use crate::index::truncate;
use crate::sidebar::{self, Row};

/// Makes muttrs' pickers known to `:pick`.
pub(super) fn add(kit: &mut Kit) {
    kit.add_picker("links", |muttrs, kit| {
        muttrs
            .links_picker(kit)
            .unwrap_or_else(|why| Picker::list("Links", Vec::new(), why))
    });
    kit.add_picker("attachments", |muttrs, kit| muttrs.attachments_picker(kit));
    kit.add_picker("mailboxes", |muttrs, kit| muttrs.mailbox_picker(kit));
    kit.add_picker("move", |muttrs, kit| muttrs.move_picker(kit));
    kit.add_picker("labels", |muttrs, kit| match muttrs.targets(kit) {
        Some(targets) => muttrs.labels_picker(&targets),
        None => Picker::list("Labels", Vec::new(), "No Message here"),
    });
}

/// Copies `url`, saying so.
pub(super) fn copy(kit: &mut Kit, url: String) -> Effects {
    kit.info(format!("Yanked {url}"));
    vec![Effect::Copy(url)]
}

impl Muttrs {
    /// The Links of the Message the Pager shows, numbered, to open in the browser, or to copy.
    pub(super) fn links_picker(&self, kit: &Kit) -> Result<Picker<Self>, String> {
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
                    pick: Pick::Target(Target::Link(link.url.clone())),
                }
            })
            .collect();
        Ok(Picker::list("Links", choices, "No Link matches").verb(
            "<C-y>",
            "yank",
            |_, kit, pick| match pick {
                Pick::Target(Target::Link(url)) => copy(kit, url),
                _ => Vec::new(),
            },
        ))
    }

    /// The Attachments of the Message the Pager shows, to open, or to save.
    fn attachments_picker(&self, kit: &Kit) -> Picker<Self> {
        let title = "Attachments";
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
                }),
            })
            .collect();
        Picker::list(title, choices, "No Attachment matches").verb(
            "<C-s>",
            "save",
            |muttrs: &mut Self, kit, pick| match pick {
                // Saved, the list goes, as muttrs' did; copying a Link leaves the Links listed, for more.
                Pick::Target(Target::Attachment { pager, part }) => {
                    kit.close_picker();
                    muttrs.attachment(pager, part, true).unwrap_or_else(|why| {
                        kit.error(why);
                        Vec::new()
                    })
                }
                _ => Vec::new(),
            },
        )
    }

    /// The Account's Mailboxes, to open one, as `:e NAME` does (`<leader>ff`, as every app finds its own).
    pub(super) fn mailbox_picker(&self, kit: &Kit) -> Picker<Self> {
        let account = self.account_shown(kit);
        let choices: Vec<Choice<Target>> =
            self.accounts.get(account).map_or_else(Vec::new, |state| {
                state
                    .mailboxes
                    .iter()
                    .filter(|mailbox| mailbox.selectable)
                    .map(|mailbox| {
                        let path = mailbox.path.join("/");
                        Choice {
                            label: vec![
                                (kit.icon(icons::role(mailbox.role)), hl::DIM),
                                (path.clone(), Group::NORMAL_FLOAT),
                            ],
                            text: path,
                            pick: Pick::Target(Target::Mailbox {
                                account,
                                name: mailbox.name.clone(),
                            }),
                        }
                    })
                    .collect()
            });
        Picker::list("Mailboxes", choices, "No Mailbox matches")
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
    pub(super) fn labels_picker(&self, targets: &Targets) -> Picker<Self> {
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
        // `<CR>` adds a Label or takes it off, the list staying open for more, as muttrs' did.
        Picker::list(title, self.label_choices(targets), "No Label matches").verb(
            "<CR>",
            "add or take off",
            |muttrs: &mut Self, kit, pick| {
                let Pick::Target(Target::Label { targets, label }) = pick else {
                    return Vec::new();
                };
                muttrs.toggle_label(&targets, label);
                let choices = muttrs.label_choices(&targets);
                if let Some(picker) = kit.picker_mut() {
                    picker.set_choices(choices);
                }
                Vec::new()
            },
        )
    }

    /// The Account's Labels, each marked as `targets` have it.
    fn label_choices(&self, targets: &Targets) -> Vec<Choice<Target>> {
        let Some(state) = self.accounts.get(targets.account) else {
            return Vec::new();
        };
        state
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
            .collect()
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
