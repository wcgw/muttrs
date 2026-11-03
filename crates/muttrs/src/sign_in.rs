//! Signing in to an Account again from the UI, with OAuth2, as `muttrs login` does from a shell: once its refresh
//! token expires, say, as Google's do after 7 days for apps it has "in testing". A popup says to sign in in the
//! browser, and takes the address the browser ends at, pasted, when it's on another machine and can't come back to
//! muttrs; meanwhile a task waits for the code, and keeps the refresh token it's exchanged for in the keyring.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::bail;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use muttrs_mail::account::{Auth, PasswordSource};
use muttrs_mail::oauth2::{self, Authorization};
use muttrs_mail::secret::{self, Kind};
use muttrs_mail::{Account, client};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget as _};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::theme::Theme;
use crate::ui::{Event, truncate};

/// The keys the popup takes, shown at its foot.
const HINT: &str = " Enter paste · ^Y copy · ^O open · Esc ";

/// What tells one sign-in's [`Progress`] from another's.
static STARTED: AtomicU64 = AtomicU64::new(0);

/// How signing in goes, as its task tells the UI, by [`Event::SignIn`].
#[derive(Debug)]
pub(crate) enum Progress {
    /// What was pasted isn't the address the browser ended at, for this reason.
    Rejected(String),
    /// Signed in, the refresh token kept in the keyring; or why not.
    Done(Result<(), String>),
}

/// What a key does to the popup.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Stay,
    /// Give up signing in.
    Cancel,
    /// Copy the provider's page's address, to open in a browser elsewhere.
    Copy,
    /// Open the provider's page in the browser again.
    Open,
}

/// Signing in to an Account, in the browser: the popup, and the task waiting for the code. Dropping it stops the
/// task, giving up signing in.
pub(crate) struct SignIn {
    /// Which sign-in this is, as its task's [`Progress`] says.
    id: u64,
    /// The Account's place in the sidebar.
    pub(crate) account: usize,
    name: String,
    /// The provider's page, where the user signs in.
    url: String,
    /// What's typed (pasted), until `Enter`.
    typed: String,
    /// Why the last address pasted isn't the one the browser ended at.
    rejected: Option<String>,
    /// Where `Enter` sends what's typed: to the task.
    pasted: mpsc::UnboundedSender<String>,
    task: AbortHandle,
}

impl SignIn {
    /// Starts signing in to `account`, the Account at `index` in the sidebar: opens the browser at its provider's
    /// page, and spawns the task that waits for the code and keeps the refresh token in the keyring, which tells
    /// `events` how it goes.
    ///
    /// # Errors
    ///
    /// If `account` doesn't sign in with OAuth2, or no loopback port can be listened on for the browser.
    pub(crate) fn start(
        index: usize,
        account: Account,
        events: mpsc::UnboundedSender<Event>,
    ) -> anyhow::Result<Self> {
        let Account {
            name, email, auth, ..
        } = account;
        let oauth2 = match auth {
            Auth::OAuth2(oauth2) => oauth2,
            Auth::Password(PasswordSource::Keyring) => {
                bail!(
                    "{name} signs in with a password: `muttrs login {name}`, in a shell, keeps it"
                )
            }
            Auth::Password(PasswordSource::Command(command)) => {
                bail!("{name} signs in with the password `{command}` prints")
            }
        };
        let authorization = Authorization::start(&oauth2, &email)?;
        let url = authorization.url().to_owned();
        let id = STARTED.fetch_add(1, Ordering::SeqCst);
        let (pasted, mut addresses) = mpsc::unbounded_channel();
        // Cloned for the task, which keeps the refresh token by the Account's name.
        let keyring_name = name.clone();
        let task = tokio::spawn(async move {
            let signed_in = async {
                let client_secret = client::client_secret(oauth2.client_secret.as_ref()).await?;
                let tokens = loop {
                    match authorization
                        .tokens(client_secret.as_deref(), &mut addresses)
                        .await
                    {
                        Err(error @ oauth2::Error::NotTheRedirect) => {
                            let _ = events
                                .send(Event::SignIn(id, Progress::Rejected(error.to_string())));
                        }
                        tokens => break tokens?,
                    }
                };
                let refresh_token = tokens
                    .refresh_token
                    .as_deref()
                    .expect("signing in gives a refresh token");
                secret::store(&keyring_name, Kind::RefreshToken, refresh_token).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            let signed_in = signed_in.map_err(|error| format!("{error:#}"));
            let _ = events.send(Event::SignIn(id, Progress::Done(signed_in)));
        });
        // Over SSH, say, there may be no browser here: the popup says what to do then.
        let _ = open::that_detached(&url);
        Ok(Self {
            id,
            account: index,
            name,
            url,
            typed: String::new(),
            rejected: None,
            pasted,
            task: task.abort_handle(),
        })
    }

    /// Whether `id` is this sign-in's, as its [`Progress`] says.
    pub(crate) fn is(&self, id: u64) -> bool {
        self.id == id
    }

    /// The provider's page, where the user signs in.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// Says why what was pasted isn't the address the browser ended at.
    pub(crate) fn reject(&mut self, why: String) {
        self.rejected = Some(why);
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> Outcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Char('c') if control => return Outcome::Cancel,
            KeyCode::Char('y') if control => return Outcome::Copy,
            KeyCode::Char('o') if control => return Outcome::Open,
            KeyCode::Char('u') if control => self.typed.clear(),
            KeyCode::Backspace => {
                self.typed.pop();
            }
            KeyCode::Enter => {
                // Unless the task is done, when what it says next closes the popup.
                let _ = self.pasted.send(std::mem::take(&mut self.typed));
                self.rejected = None;
            }
            KeyCode::Char(character) if !control => self.typed.push(character),
            _ => {}
        }
        Outcome::Stay
    }

    /// Draws the popup in the middle of `area`, the cursor where what's pasted goes.
    pub(crate) fn draw(&self, area: Rect, frame: &mut Frame<'_>, theme: &Theme) {
        let width = area.width.saturating_sub(4).min(68).max(
            u16::try_from(HINT.width() + 4)
                .unwrap_or(u16::MAX)
                .min(area.width),
        );
        // Within the borders, a cell from each.
        let room = usize::from(width.saturating_sub(4));
        let mut lines = Vec::new();
        let say = |lines: &mut Vec<Line<'static>>, text: &str, style: Style| {
            lines.extend(
                wrap(text, room)
                    .into_iter()
                    .map(|line| Line::styled(line, style)),
            );
        };
        say(
            &mut lines,
            "Sign in in the browser: muttrs carries on once you have.",
            Style::new(),
        );
        lines.push(Line::default());
        say(
            &mut lines,
            "A browser on another machine can't come back here: open this page there (^Y copies it), then paste \
             the address the browser ends at.",
            Style::new(),
        );
        lines.push(Line::styled(truncate(&self.url, room), theme.dim));
        lines.push(Line::default());
        let typed = tail(&self.typed, room.saturating_sub(2));
        let field = lines.len();
        lines.push(Line::from(vec![
            Span::styled("› ", theme.accent),
            Span::raw(typed.clone()),
        ]));
        if let Some(why) = &self.rejected {
            say(&mut lines, why, theme.error);
        }

        let height = u16::try_from(lines.len() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let popup = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width.min(area.width),
            height,
        );
        let buffer = frame.buffer_mut();
        Clear.render(popup, buffer);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.accent)
            .title(Span::styled(
                format!(" Signing in to {} ", self.name),
                theme.accent,
            ))
            .title_bottom(Line::styled(HINT, theme.dim).right_aligned());
        let inner = block.inner(popup);
        block.render(popup, buffer);
        for (row, line) in lines.iter().enumerate().take(usize::from(inner.height)) {
            // Within the popup's u16 height, by the take.
            let y = inner.y + u16::try_from(row).unwrap_or(0);
            buffer.set_line(inner.x + 1, y, line, inner.width.saturating_sub(2));
        }
        if let Ok(row) = u16::try_from(field)
            && row < inner.height
        {
            let x = inner.x + 3 + u16::try_from(typed.width()).unwrap_or(0);
            frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y + row));
        }
    }
}

impl Drop for SignIn {
    /// Stops the task: closing the popup gives up signing in.
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `text` in lines of at most `width` cells, broken between words (a word wider than that has a line of its own).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The end of `text` that fits in `width` cells, after an ellipsis if it's cut: what's typed last shows.
fn tail(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut kept = 0;
    let mut start = text.len();
    for (at, character) in text.char_indices().rev() {
        let cells = character.width().unwrap_or(0);
        if kept + cells + 1 > width {
            break;
        }
        kept += cells;
        start = at;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_wrap_and_the_end_of_what_is_typed_shows() {
        assert_eq!(
            wrap("paste the address  it ends at", 12),
            ["paste the", "address it", "ends at"]
        );
        assert_eq!(wrap("", 12), Vec::<String>::new());
        assert_eq!(tail("127.0.0.1:4242/?code=x", 30), "127.0.0.1:4242/?code=x");
        assert_eq!(tail("127.0.0.1:4242/?code=x", 8), "…?code=x");
        assert_eq!(tail("ab", 0), "…");
    }
}
