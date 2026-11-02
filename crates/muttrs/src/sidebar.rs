//! The sidebar: each Account, then its Mailboxes, those with a Role first, and their Unread counts.

use std::collections::HashMap;

use muttrs_mail::client::{Client, Connection};
use muttrs_mail::{Counts, Mailbox, Role};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr as _;

use crate::icons::Icons;
use crate::theme::Theme;
use crate::ui::truncate;

/// An Account, as the UI knows it.
pub(crate) struct AccountState {
    pub(crate) name: String,
    pub(crate) client: Client,
    pub(crate) connection: Connection,
    pub(crate) mailboxes: Vec<Mailbox>,
    pub(crate) counts: HashMap<String, Counts>,
    /// Who it sends as.
    pub(crate) sender: muttrs_mail::compose::Sender,
}

impl AccountState {
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
    Account(usize),
    Mailbox(usize, String),
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

/// The rows the sidebar lists: each Account, its Mailboxes with a Role, then the others in their hierarchy.
pub(crate) fn rows(accounts: &[AccountState]) -> Vec<Row> {
    rows_of(accounts.iter().map(|account| account.mailboxes.as_slice()))
}

/// The rows of Accounts that have these Mailboxes.
fn rows_of<'a>(accounts: impl Iterator<Item = &'a [Mailbox]>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, mailboxes) in accounts.enumerate() {
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
        rows(accounts).into_iter().nth(self.cursor)
    }

    pub(crate) fn move_by(&mut self, accounts: &[AccountState], by: isize) {
        let count = rows(accounts).len();
        self.cursor = self
            .cursor
            .saturating_add_signed(by)
            .min(count.saturating_sub(1));
    }

    pub(crate) fn top(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn bottom(&mut self, accounts: &[AccountState]) {
        self.cursor = rows(accounts).len().saturating_sub(1);
    }

    /// The row drawn `offset` rows down from the top of the sidebar, the cursor put on it.
    pub(crate) fn click(&mut self, accounts: &[AccountState], offset: usize) -> Option<Row> {
        let row = self.top + offset;
        let found = rows(accounts).into_iter().nth(row)?;
        self.cursor = row;
        Some(found)
    }

    /// Puts the cursor on `row`, if it's listed.
    pub(crate) fn select(&mut self, accounts: &[AccountState], row: &Row) {
        if let Some(at) = rows(accounts).iter().position(|listed| listed == row) {
            self.cursor = at;
        }
    }

    pub(crate) fn draw(&mut self, area: Rect, buffer: &mut Buffer, cx: &Draw<'_>) {
        let rows = rows(cx.accounts);
        self.cursor = self.cursor.min(rows.len().saturating_sub(1));
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
    let unread = counts.map_or(0, |counts| counts.unread);
    let style = if !mailbox.selectable {
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
    let name = format!("{}{}", icon(cx.icons, mailbox.role), mailbox.label());
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
        let listed: Vec<String> = rows_of(std::iter::once(gmail.as_slice()))
            .into_iter()
            .map(|row| match row {
                Row::Account(_) => "account".to_owned(),
                Row::Mailbox(_, name) => name,
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
}
