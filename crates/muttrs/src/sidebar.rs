//! The sidebar: each Account, then its Mailboxes, those with a Role first, and their Unread counts.

use std::collections::HashMap;
use std::time::Instant;

use muttrs_mail::client::{Client, Connection, Request, Task};
use muttrs_mail::{Counts, Mailbox, Role};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr as _;

use crate::icons::Icons;
use crate::tasks::Tasks;
use crate::theme::Theme;
use crate::ui::truncate;

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
    /// The heading of the Accounts' Inboxes, listed first when there are several Accounts: it's not chosen.
    Inboxes,
    /// An Account's Inbox, under `Inboxes`, by the Account's name.
    Inbox(usize),
    Account(usize),
    Mailbox(usize, String),
}

impl Row {
    /// The Mailbox it opens, by its Account and its name on the server.
    pub(crate) fn opens(&self, accounts: &[AccountState]) -> Option<(usize, String)> {
        match self {
            Self::Mailbox(account, name) => Some((*account, name.clone())),
            Self::Inbox(account) => Some((*account, inbox_of(accounts.get(*account)?))),
            Self::Inboxes | Self::Account(_) => None,
        }
    }

    /// Whether the cursor can be on it: all but the heading.
    fn is_chosen(&self) -> bool {
        !matches!(self, Self::Inboxes)
    }
}

/// The name on the server of `account`'s Inbox: `INBOX`, as IMAP names it, until the server lists it.
fn inbox_of(account: &AccountState) -> String {
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

/// The icon of a Mailbox with `role`.
pub(crate) fn icon(icons: &Icons, role: Option<Role>) -> &'static str {
    match role {
        Some(Role::Inbox) => icons.inbox,
        Some(Role::Sent) => icons.sent,
        Some(Role::Drafts) => icons.drafts,
        Some(Role::Trash) => icons.trash,
        Some(Role::Junk) => icons.junk,
        Some(Role::Archive) => icons.archive,
        Some(Role::All) => icons.all,
        Some(Role::Flagged) => icons.flagged,
        None => icons.folder,
    }
}

#[derive(Debug, Default)]
pub(crate) struct Sidebar {
    cursor: usize,
    top: usize,
}

/// What drawing the sidebar takes.
pub(crate) struct Draw<'a> {
    pub(crate) accounts: &'a [AccountState],
    /// The Mailbox the Index shows.
    pub(crate) open: Option<(usize, &'a str)>,
    pub(crate) focused: bool,
    pub(crate) theme: &'a Theme,
    pub(crate) icons: &'a Icons,
}

impl Sidebar {
    /// The row under the cursor.
    pub(crate) fn current(&self, accounts: &[AccountState]) -> Option<Row> {
        let rows = rows(accounts);
        rows.get(settled(&rows, self.cursor)).cloned()
    }

    pub(crate) fn move_by(&mut self, accounts: &[AccountState], by: isize) {
        self.cursor = moved(&rows(accounts), self.cursor, by);
    }

    pub(crate) fn top(&mut self, accounts: &[AccountState]) {
        self.cursor = settled(&rows(accounts), 0);
    }

    pub(crate) fn bottom(&mut self, accounts: &[AccountState]) {
        let rows = rows(accounts);
        self.cursor = settled(&rows, rows.len().saturating_sub(1));
    }

    /// The row drawn `offset` rows down from the top of the sidebar, the cursor put on it; None for the heading,
    /// which isn't chosen.
    pub(crate) fn click(&mut self, accounts: &[AccountState], offset: usize) -> Option<Row> {
        let row = self.top + offset;
        let found = rows(accounts).into_iter().nth(row)?;
        if !found.is_chosen() {
            return None;
        }
        self.cursor = row;
        Some(found)
    }

    /// Puts the cursor on `row`, if it's listed; unless it's on a row that opens the same Mailbox already, as an
    /// Account's under the Inboxes does its Inbox.
    pub(crate) fn select(&mut self, accounts: &[AccountState], row: &Row) {
        let rows = rows(accounts);
        let opens = row.opens(accounts);
        if opens.is_some()
            && rows
                .get(settled(&rows, self.cursor))
                .is_some_and(|current| current.opens(accounts) == opens)
        {
            return;
        }
        if let Some(at) = rows.iter().position(|listed| listed == row) {
            self.cursor = at;
        }
    }

    pub(crate) fn draw(&mut self, area: Rect, buffer: &mut Buffer, cx: &Draw<'_>) {
        let rows = rows(cx.accounts);
        self.cursor = settled(&rows, self.cursor);
        let height = usize::from(area.height);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if height > 0 && self.cursor >= self.top + height {
            self.top = self.cursor + 1 - height;
        }
        let width = usize::from(area.width.saturating_sub(1));
        for (index, row) in rows.iter().enumerate().skip(self.top).take(height) {
            let y = area.y + u16::try_from(index - self.top).unwrap_or(0);
            let (line, open) = match row {
                Row::Inboxes => (
                    Line::from(Span::styled(
                        format!("{}Inboxes", cx.icons.inbox),
                        cx.theme.accent,
                    )),
                    false,
                ),
                Row::Inbox(account) => {
                    let state = &cx.accounts[*account];
                    let inbox = inbox_of(state);
                    let open = cx.open == Some((*account, inbox.as_str()));
                    let unread = state.counts.get(&inbox).map_or(0, |counts| counts.unread);
                    (
                        entry_line(1, cx.icons.account, &state.name, true, unread, width, cx),
                        open,
                    )
                }
                Row::Account(account) => (Self::account_line(&cx.accounts[*account], cx), false),
                Row::Mailbox(account, name) => {
                    let state = &cx.accounts[*account];
                    let Some(mailbox) = state.mailbox(name) else {
                        continue;
                    };
                    let open = cx.open == Some((*account, name.as_str()));
                    (
                        mailbox_line(mailbox, state.counts.get(name), width, cx),
                        open,
                    )
                }
            };
            buffer.set_line(area.x, y, &line, area.width.saturating_sub(1));
            let row_area = Rect::new(area.x, y, area.width.saturating_sub(1), 1);
            if index == self.cursor && cx.focused {
                buffer.set_style(row_area, cx.theme.cursor);
            } else if open {
                buffer.set_style(row_area, cx.theme.cursor_unfocused);
            }
        }
        // A rule between the sidebar and the Index.
        for y in area.y..area.y + area.height {
            buffer.set_string(area.x + area.width.saturating_sub(1), y, "│", cx.theme.dim);
        }
    }

    fn account_line(account: &AccountState, cx: &Draw<'_>) -> Line<'static> {
        let (mark, style) = match &account.connection {
            Connection::Online => (cx.icons.online, cx.theme.dim),
            Connection::Connecting => (cx.icons.loading, cx.theme.dim),
            Connection::Offline(_) => (cx.icons.offline, cx.theme.error),
            Connection::SignInFailed(_) => (cx.icons.alert, cx.theme.error),
        };
        Line::from(vec![
            Span::styled(
                format!("{}{}", cx.icons.account, account.name),
                cx.theme.accent,
            ),
            Span::raw(" "),
            Span::styled(mark.trim_end().to_owned(), style),
        ])
    }
}

fn mailbox_line(
    mailbox: &Mailbox,
    counts: Option<&Counts>,
    width: usize,
    cx: &Draw<'_>,
) -> Line<'static> {
    let indent = if mailbox.role.is_some() {
        1
    } else {
        1 + mailbox.depth() * 2
    };
    entry_line(
        indent,
        icon(cx.icons, mailbox.role),
        mailbox.label(),
        mailbox.selectable,
        counts.map_or(0, |counts| counts.unread),
        width,
        cx,
    )
}

/// A row of a Mailbox, `indent`ed, `width` cells wide: its icon and `label`, then how many are `unread`, if any.
fn entry_line(
    indent: usize,
    icon: &str,
    label: &str,
    selectable: bool,
    unread: u32,
    width: usize,
    cx: &Draw<'_>,
) -> Line<'static> {
    let style = if !selectable {
        cx.theme.dim
    } else if unread > 0 {
        cx.theme.unread
    } else {
        cx.theme.read
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
    Line::from(vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(name, style),
        Span::raw(" ".repeat(padding)),
        Span::styled(
            count,
            if unread > 0 {
                cx.theme.accent
            } else {
                Style::new()
            },
        ),
    ])
}

/// Where the cursor at `cursor` among `rows` settles: on it, unless it's the heading, then on the row after (or
/// before, at the end).
fn settled(rows: &[Row], cursor: usize) -> usize {
    let cursor = cursor.min(rows.len().saturating_sub(1));
    (cursor..rows.len())
        .chain((0..cursor).rev())
        .find(|&at| rows[at].is_chosen())
        .unwrap_or(cursor)
}

/// Where the cursor at `cursor` among `rows` goes, moved `by` rows of those it can be on.
fn moved(rows: &[Row], cursor: usize, by: isize) -> usize {
    let chosen: Vec<usize> = (0..rows.len()).filter(|&at| rows[at].is_chosen()).collect();
    let Some(now) = chosen.iter().position(|&at| at >= settled(rows, cursor)) else {
        return cursor;
    };
    chosen[now
        .saturating_add_signed(by)
        .min(chosen.len().saturating_sub(1))]
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
    fn several_accounts_inboxes_come_first_under_a_heading_the_cursor_passes_over() {
        let home = listed(&[("INBOX", &[]), ("Archive", &[])]);
        let work = listed(&[("INBOX", &[])]);
        let rows = rows_of(&[home.as_slice(), work.as_slice()]);
        assert_eq!(
            rows[..4],
            [Row::Inboxes, Row::Inbox(0), Row::Inbox(1), Row::Account(0)]
        );
        assert_eq!(settled(&rows, 0), 1, "not on the heading");
        assert_eq!(moved(&rows, 1, -1), 1, "nor up onto it");
        assert_eq!(moved(&rows, 0, 1), 2);
        assert_eq!(moved(&rows, 2, 1), 3, "then the Accounts, as before");
        assert_eq!(moved(&rows, 1, 100), rows.len() - 1);
        assert!(
            !rows_of(&[home.as_slice()]).contains(&Row::Inboxes),
            "one Account: no Inboxes"
        );
    }
}
