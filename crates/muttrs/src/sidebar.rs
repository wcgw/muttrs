//! The sidebar: each Account, then its Mailboxes, those with a Role first, and their Unread counts. It's an unlisted
//! kit buffer in a window of its own at the left: a block per Account, folded by `zc` into the Account's row, and a
//! fold over each Mailbox holding others.

use std::collections::HashMap;
use std::time::Instant;

use muttrs_mail::client::{Client, Connection, Request, Task};
use muttrs_mail::{Counts, Mailbox, Role};
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr as _;
use wcgw_kit::doc::{BlockKey, Content, Fold};

use crate::app::{Line, Span, Target};
use crate::hl::{self, Group};
use crate::icons;
use crate::index::truncate;
use crate::tasks::Tasks;

/// An Account, as the UI knows it.
pub(crate) struct AccountState {
    pub(crate) name: String,
    pub(crate) client: Client,
    pub(crate) connection: Connection,
    pub(crate) mailboxes: Vec<Mailbox>,
    /// Its Labels, by name, as the server last told them.
    pub(crate) labels: Vec<muttrs_mail::Label>,
    pub(crate) counts: HashMap<String, Counts>,
    /// Who it sends as.
    pub(crate) sender: muttrs_mail::compose::Sender,
    /// What's kept of it on disk: what shows before the server says.
    pub(crate) cache: muttrs_mail::cache::Cache,
    /// What's asked of it and not finished.
    pub(crate) tasks: Tasks,
}

impl AccountState {
    /// Asks the Account for `request`, keeping it among its Tasks until it's finished.
    pub(crate) fn ask(&mut self, request: Request) -> Task {
        self.ask_as(request, None)
    }

    /// Asks the Account for `request`, said on the status line as `what`, else as the request says.
    pub(crate) fn ask_as(&mut self, request: Request, what: Option<String>) -> Task {
        self.tasks.ask(
            &self.client,
            request,
            what,
            (&self.mailboxes, &self.labels),
            Instant::now(),
        )
    }

    /// The Mailbox with `role`, by its name on the server.
    pub(crate) fn with_role(&self, role: Role) -> Option<&Mailbox> {
        self.mailboxes
            .iter()
            .find(|mailbox| mailbox.role == Some(role))
    }

    pub(crate) fn mailbox(&self, name: &str) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|mailbox| mailbox.name == name)
    }
}

/// A row of the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// The heading of the Accounts' Inboxes, listed first when there are several Accounts.
    Inboxes,
    /// An Account's Inbox, under `Inboxes`, by the Account's name.
    Inbox(usize),
    Account(usize),
    Mailbox(usize, String),
}

/// The name on the server of `account`'s Inbox: `INBOX`, as IMAP names it, until the server lists it.
pub(crate) fn inbox_of(account: &AccountState) -> String {
    account
        .with_role(Role::Inbox)
        .map_or_else(|| "INBOX".to_owned(), |inbox| inbox.name.clone())
}

/// The order Roles are listed in.
fn role_order(role: Role) -> u8 {
    match role {
        Role::Inbox => 0,
        Role::Flagged => 1,
        Role::Drafts => 2,
        Role::Sent => 3,
        Role::Archive => 4,
        Role::All => 5,
        Role::Junk => 6,
        Role::Trash => 7,
    }
}

/// The rows the sidebar lists: with several Accounts, their Inboxes first; then each Account, its Mailboxes with a
/// Role, then the others in their hierarchy.
pub(crate) fn rows(accounts: &[AccountState]) -> Vec<Row> {
    let mailboxes: Vec<&[Mailbox]> = accounts
        .iter()
        .map(|account| account.mailboxes.as_slice())
        .collect();
    rows_of(&mailboxes)
}

/// The rows of Accounts that have these Mailboxes.
fn rows_of(accounts: &[&[Mailbox]]) -> Vec<Row> {
    let mut rows = Vec::new();
    if accounts.len() > 1 {
        rows.push(Row::Inboxes);
        rows.extend((0..accounts.len()).map(Row::Inbox));
    }
    for (index, mailboxes) in accounts.iter().copied().enumerate() {
        rows.push(Row::Account(index));
        let mut roles: Vec<&Mailbox> = mailboxes.iter().filter(|m| m.role.is_some()).collect();
        roles.sort_by_key(|mailbox| mailbox.role.map(role_order));
        let mut others: Vec<&Mailbox> = mailboxes
            .iter()
            .filter(|mailbox| mailbox.role.is_none())
            // A parent that can't be opened shows only when it holds more than Mailboxes with Roles, as
            // `[Gmail]` doesn't.
            .filter(|mailbox| {
                mailbox.selectable
                    || mailboxes.iter().any(|child| {
                        child.role.is_none()
                            && child.path.len() > mailbox.path.len()
                            && child.path.starts_with(&mailbox.path)
                    })
            })
            .collect();
        others.sort_by(|a, b| a.path.cmp(&b.path));
        rows.extend(
            roles
                .into_iter()
                .chain(others)
                .map(|mailbox| Row::Mailbox(index, mailbox.name.clone())),
        );
    }
    rows
}

/// The key of the block of `row`'s Account, or of the Inboxes.
fn block_of(row: &Row) -> String {
    match row {
        Row::Inboxes | Row::Inbox(_) => "inboxes".to_owned(),
        Row::Account(account) | Row::Mailbox(account, _) => format!("account{account}"),
    }
}

/// What drawing the sidebar takes.
pub(crate) struct Draw<'a> {
    pub(crate) accounts: &'a [AccountState],
    /// The Mailbox the Index shows.
    pub(crate) open: Option<(usize, &'a str)>,
    pub(crate) width: usize,
    /// The open Mailbox's row's colours.
    pub(crate) open_style: Style,
    pub(crate) icon: &'a dyn Fn(&str) -> String,
}

/// The sidebar's blocks: the Inboxes, if there are several Accounts, then each Account with its Mailboxes. Each is
/// folded into its first row by `zc`, and so is each Mailbox holding others into its own.
pub(crate) fn blocks(cx: &Draw<'_>) -> Vec<(BlockKey, Content<Target>)> {
    let rows = rows(cx.accounts);
    let mut blocks: Vec<(String, Vec<Row>)> = Vec::new();
    for row in rows {
        let key = block_of(&row);
        match blocks.last_mut() {
            Some((last, rows)) if *last == key => rows.push(row),
            _ => blocks.push((key, vec![row])),
        }
    }
    blocks
        .into_iter()
        .map(|(key, rows)| {
            let lines: Vec<Line> = rows.iter().map(|row| line(row, cx)).collect();
            let mut folds = Vec::new();
            if rows.len() > 1 {
                folds.push(Fold {
                    lines: 0..rows.len(),
                    summary: folded(&rows[0], rows.len() - 1, cx),
                    closed: false,
                });
            }
            // A Mailbox holding others, folded into its row.
            for (at, row) in rows.iter().enumerate() {
                let Row::Mailbox(account, name) = row else {
                    continue;
                };
                let Some(parent) = cx.accounts[*account].mailbox(name) else {
                    continue;
                };
                if parent.role.is_some() {
                    continue;
                }
                let children = rows[at + 1..]
                    .iter()
                    .take_while(|row| match row {
                        Row::Mailbox(of, child) => {
                            cx.accounts[*of].mailbox(child).is_some_and(|child| {
                                child.role.is_none()
                                    && child.path.len() > parent.path.len()
                                    && child.path.starts_with(&parent.path)
                            })
                        }
                        _ => false,
                    })
                    .count();
                if children > 0 {
                    folds.push(Fold {
                        lines: at..at + 1 + children,
                        summary: folded(row, children, cx),
                        closed: false,
                    });
                }
            }
            let content = Content {
                lines,
                folds,
                target: None,
            };
            (BlockKey::new(key), content)
        })
        .collect()
}

/// `row`, folded with the `hidden` rows under it: as it is, saying how many it hides.
fn folded(row: &Row, hidden: usize, cx: &Draw<'_>) -> Line {
    let Line::Text(mut spans) = line(row, cx) else {
        return line(row, cx);
    };
    let more = format!(" +{hidden}");
    // In place of what pads the row, so it stays as wide.
    if let Some(padding) = spans
        .iter()
        .position(|span| span.text.trim().is_empty() && span.text.len() > more.len())
    {
        let width = spans[padding].text.width() - more.width();
        spans[padding] = Span::new(" ".repeat(width), Group::NORMAL);
        spans.insert(padding, Span::new(more, hl::DIM));
    } else {
        spans.push(Span::new(more, hl::DIM));
    }
    Line::Text(spans)
}

/// The line of `row`, as wide as the sidebar.
fn line(row: &Row, cx: &Draw<'_>) -> Line {
    let (spans, open, target) = match row {
        Row::Inboxes => (
            vec![Span::new(
                format!("{}Inboxes", (cx.icon)(icons::INBOX)),
                hl::ACCENT,
            )],
            false,
            None,
        ),
        Row::Inbox(account) => {
            let state = &cx.accounts[*account];
            let inbox = inbox_of(state);
            let open = cx.open == Some((*account, inbox.as_str()));
            let unread = state.counts.get(&inbox).map_or(0, |counts| counts.unread);
            (
                entry(
                    1,
                    &(cx.icon)(icons::ACCOUNT),
                    &state.name,
                    true,
                    unread,
                    cx.width,
                ),
                open,
                Some(Target::Mailbox {
                    account: *account,
                    name: inbox,
                }),
            )
        }
        Row::Account(account) => (
            account_line(&cx.accounts[*account], cx),
            false,
            Some(Target::Account(*account)),
        ),
        Row::Mailbox(account, name) => {
            let state = &cx.accounts[*account];
            let open = cx.open == Some((*account, name.as_str()));
            let spans = match state.mailbox(name) {
                Some(mailbox) => {
                    let indent = if mailbox.role.is_some() {
                        1
                    } else {
                        1 + mailbox.depth() * 2
                    };
                    entry(
                        indent,
                        &(cx.icon)(icons::role(mailbox.role)),
                        mailbox.label(),
                        mailbox.selectable,
                        state.counts.get(name).map_or(0, |counts| counts.unread),
                        cx.width,
                    )
                }
                None => vec![Span::new(name.clone(), hl::DIM)],
            };
            (
                spans,
                open,
                Some(Target::Mailbox {
                    account: *account,
                    name: name.clone(),
                }),
            )
        }
    };
    Line::Text(
        spans
            .into_iter()
            .map(|span| {
                let span = match &target {
                    Some(target) => span.with_target(target.clone()),
                    None => span,
                };
                if open {
                    span.with_style(cx.open_style)
                } else {
                    span
                }
            })
            .collect(),
    )
}

fn account_line(account: &AccountState, cx: &Draw<'_>) -> Vec<Span> {
    let (mark, group) = match &account.connection {
        Connection::Online => (icons::ONLINE, hl::DIM),
        Connection::Connecting => (icons::LOADING, hl::DIM),
        Connection::Offline(_) => (icons::OFFLINE, hl::ERROR),
        Connection::SignInFailed(_) => (icons::ALERT, hl::ERROR),
    };
    let name = format!("{}{}", (cx.icon)(icons::ACCOUNT), account.name);
    let mark = (cx.icon)(mark);
    let used = name.width() + 1 + mark.width();
    vec![
        Span::new(name, hl::ACCENT),
        Span::new(" ", Group::NORMAL),
        Span::new(mark, group),
        Span::new(" ".repeat(cx.width.saturating_sub(used)), Group::NORMAL),
    ]
}

/// A row of a Mailbox, `indent`ed, `width` cells wide: its icon and `label`, then how many are `unread`, if any.
fn entry(
    indent: usize,
    icon: &str,
    label: &str,
    selectable: bool,
    unread: u32,
    width: usize,
) -> Vec<Span> {
    let group = if !selectable {
        hl::DIM
    } else if unread > 0 {
        hl::UNREAD
    } else {
        hl::READ
    };
    let count = if unread > 0 {
        format!(" {unread}")
    } else {
        String::new()
    };
    let name = format!("{icon}{label}");
    let room = width.saturating_sub(indent + count.width());
    let name = truncate(&name, room);
    let padding = width.saturating_sub(indent + name.width() + count.width());
    vec![
        Span::new(" ".repeat(indent), Group::NORMAL),
        Span::new(name, group),
        Span::new(" ".repeat(padding), Group::NORMAL),
        Span::new(count, hl::ACCENT),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(mailboxes: &[(&str, &[&str])]) -> Vec<Mailbox> {
        mailboxes
            .iter()
            .map(|(name, attributes)| Mailbox::listed(name, Some("/"), attributes))
            .collect()
    }

    #[test]
    fn roles_first_then_the_hierarchy_without_empty_parents() {
        let gmail = listed(&[
            ("INBOX", &[]),
            ("Work", &[]),
            ("Work/Reports", &[]),
            ("[Gmail]", &["\\Noselect"]),
            ("[Gmail]/All Mail", &["\\All"]),
            ("[Gmail]/Sent Mail", &["\\Sent"]),
            ("[Gmail]/Trash", &["\\Trash"]),
            ("Archive", &[]),
        ]);
        let listed: Vec<String> = rows_of(&[gmail.as_slice()])
            .into_iter()
            .map(|row| match row {
                Row::Account(_) => "account".to_owned(),
                Row::Mailbox(_, name) => name,
                Row::Inboxes | Row::Inbox(_) => unreachable!("one Account only"),
            })
            .collect();
        assert_eq!(
            listed,
            [
                "account",
                "INBOX",
                "[Gmail]/Sent Mail",
                "[Gmail]/All Mail",
                "[Gmail]/Trash",
                "Archive",
                "Work",
                "Work/Reports"
            ]
        );
    }

    #[test]
    fn several_accounts_inboxes_come_first_under_a_heading() {
        let home = listed(&[("INBOX", &[]), ("Archive", &[])]);
        let work = listed(&[("INBOX", &[])]);
        let rows = rows_of(&[home.as_slice(), work.as_slice()]);
        assert_eq!(
            rows[..4],
            [Row::Inboxes, Row::Inbox(0), Row::Inbox(1), Row::Account(0)]
        );
        assert!(
            !rows_of(&[home.as_slice()]).contains(&Row::Inboxes),
            "one Account: no Inboxes"
        );
    }
}
