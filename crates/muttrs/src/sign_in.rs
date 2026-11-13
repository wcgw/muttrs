//! Signing in to an Account again from the UI, with OAuth2, as `muttrs login` does from a shell: once its refresh
//! token expires, say, as Google's do after 7 days for apps it has "in testing". A buffer says to sign in in the
//! browser, and a prompt takes the address the browser ends at, pasted, when it's on another machine and can't come
//! back to muttrs; meanwhile a task waits for the code, and keeps the refresh token it's exchanged for in the keyring.

use std::sync::Arc;

use anyhow::bail;
use muttrs_mail::account::{Auth, PasswordSource};
use muttrs_mail::oauth2::{self, Authorization};
use muttrs_mail::secret::{self, Kind};
use muttrs_mail::{Account, client};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

/// How signing in goes, as its task tells the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    /// The browser was opened at the provider's page, at this address.
    Started(String),
    /// What was pasted isn't the address the browser ended at, for this reason.
    Rejected(String),
    /// Signed in, the refresh token kept in the keyring; or why not.
    Done(Result<(), String>),
}

/// Signing in to an Account, in the browser: the task waiting for the code. Dropping it stops the task, giving up
/// signing in.
#[derive(Debug)]
pub(crate) struct SignIn {
    /// Where pasted addresses go: to the task.
    pasted: mpsc::UnboundedSender<String>,
    task: AbortHandle,
}

impl SignIn {
    /// Starts signing in to `account`: opens the browser at its provider's page, and spawns the task that waits for
    /// the code and keeps the refresh token in the keyring, which tells `report` how it goes.
    ///
    /// # Errors
    ///
    /// If `account` doesn't sign in with OAuth2, or no loopback port can be listened on for the browser.
    pub(crate) fn start(
        account: Account,
        report: impl Fn(Progress) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        let report = Arc::new(report);
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
        let (pasted, mut addresses) = mpsc::unbounded_channel();
        let reporting = Arc::clone(&report);
        let task = tokio::spawn(async move {
            let signed_in = async {
                let client_secret = client::client_secret(oauth2.client_secret.as_ref()).await?;
                let tokens = loop {
                    match authorization
                        .tokens(client_secret.as_deref(), &mut addresses)
                        .await
                    {
                        Err(error @ oauth2::Error::NotTheRedirect) => {
                            reporting(Progress::Rejected(error.to_string()));
                        }
                        tokens => break tokens?,
                    }
                };
                let refresh_token = tokens
                    .refresh_token
                    .as_deref()
                    .expect("signing in gives a refresh token");
                secret::store(&name, Kind::RefreshToken, refresh_token).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            reporting(Progress::Done(
                signed_in.map_err(|error| format!("{error:#}")),
            ));
        });
        // Over SSH, say, there may be no browser here: the buffer says what to do then.
        let _ = open::that_detached(&url);
        report(Progress::Started(url));
        Ok(Self {
            pasted,
            task: task.abort_handle(),
        })
    }

    /// Hands the task the address the browser ended at, pasted.
    pub(crate) fn paste(&self, address: String) {
        // Unless the task is done, when what it said is on its way.
        let _ = self.pasted.send(address);
    }
}

impl Drop for SignIn {
    /// Stops the task: giving up signing in.
    fn drop(&mut self) {
        self.task.abort();
    }
}
