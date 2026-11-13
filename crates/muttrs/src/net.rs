//! muttrs' side of the kit's runtime (its D2): what muttrs asks besides what its Accounts do, carried out in the
//! background, each telling the app how it went through the kit's inbox; and the Pictures' bytes for the kit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use wcgw_kit::runtime::{Bytes, Inbox, Perform};

use crate::app::{Io, Msg, Muttrs};
use crate::pager::Pictures;
use crate::remote::Fetcher;
use crate::sign_in::SignIn;

/// What carries out muttrs' effects.
pub(crate) struct Net {
    inbox: Inbox<Msg>,
    /// Remote Content and unsubscribing, if an HTTP client could be set up.
    fetcher: Option<Fetcher>,
    /// The Accounts as configured, to sign in to again.
    accounts: Vec<muttrs_mail::AccountConfig>,
    /// The sign-ins under way, by their IDs.
    sign_ins: HashMap<u64, SignIn>,
    /// The Pictures' bytes, as the Pagers keep them.
    pictures: Pictures,
    /// voight, if it's on.
    #[cfg(feature = "voight")]
    assessor: Option<crate::assessment::Assessor>,
}

impl Net {
    pub(crate) fn new(
        inbox: Inbox<Msg>,
        accounts: Vec<muttrs_mail::AccountConfig>,
        pictures: Pictures,
    ) -> Self {
        let fetcher = match Fetcher::new() {
            Ok(fetcher) => Some(fetcher),
            Err(error) => {
                tracing::warn!(%error, "Remote Content can't be fetched");
                None
            }
        };
        Self {
            inbox,
            fetcher,
            accounts,
            sign_ins: HashMap::new(),
            pictures,
            #[cfg(feature = "voight")]
            assessor: None,
        }
    }

    /// With voight's Assessor, to assess what the Pager shows.
    #[cfg(feature = "voight")]
    pub(crate) fn with_assessor(self, assessor: Option<crate::assessment::Assessor>) -> Self {
        Self { assessor, ..self }
    }

    fn send(&self, msg: Msg) {
        // Fails only once the runtime has ended, when nobody needs the news.
        let _ = self.inbox.send(msg);
    }

    /// Fetches the Remote Content at `url` for the image at `source`: its bytes go with the Pictures, its size to
    /// the app.
    fn remote(&self, source: String, url: String) {
        let Some(fetcher) = self.fetcher.clone() else {
            self.send(Msg::Fetched {
                source,
                pixels: None,
            });
            return;
        };
        let (inbox, pictures) = (self.inbox.clone(), Arc::clone(&self.pictures));
        tokio::spawn(async move {
            let pixels = match fetcher.image(&url).await {
                Ok(bytes) => {
                    let pixels = crate::pager::dimensions(&bytes);
                    if pixels.is_some() {
                        pictures
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(source.clone(), Arc::new(bytes));
                    }
                    pixels
                }
                Err(error) => {
                    tracing::debug!(%error, "Remote Content couldn't be fetched");
                    None
                }
            };
            let _ = inbox.send(Msg::Fetched { source, pixels });
        });
    }

    fn unsubscribe(&self, url: String, list: String) {
        let Some(fetcher) = self.fetcher.clone() else {
            self.send(Msg::Unsubscribed(Err(
                "Not unsubscribed: there's no HTTP client".to_owned(),
            )));
            return;
        };
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            let unsubscribed = match fetcher.unsubscribe(&url).await {
                Ok(()) => Ok(format!("Unsubscribed from {list}")),
                Err(error) => Err(format!("Not unsubscribed from {list}: {error:#}")),
            };
            let _ = inbox.send(Msg::Unsubscribed(unsubscribed));
        });
    }

    fn sign_in(&mut self, id: u64, account: &str) {
        let inbox = self.inbox.clone();
        let report = move |progress| {
            let _ = inbox.send(Msg::SignIn(id, progress));
        };
        match crate::login::resolve(&self.accounts, account)
            .and_then(|account| SignIn::start(account, report))
        {
            Ok(sign_in) => {
                self.sign_ins.insert(id, sign_in);
            }
            Err(error) => self.send(Msg::SignIn(
                id,
                crate::sign_in::Progress::Done(Err(format!("{error:#}"))),
            )),
        }
    }
}

/// Saves `bytes` as `name` in `directory`, under a name of its own if that one's taken; where it went.
fn save(directory: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let mut path = directory.join(name);
    let mut copy = 1;
    while path.exists() {
        path = directory.join(format!("{stem} ({copy}){extension}"));
        copy += 1;
    }
    std::fs::write(&path, bytes).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

/// Writes `bytes` to a file of muttrs' own, and opens it with what the system opens such files with.
fn open_attachment(name: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let directory = crate::xdg::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("attachments")
        .join(std::process::id().to_string());
    let path = save(&directory, name, bytes)?;
    open::that_detached(&path).with_context(|| format!("opening {}", path.display()))?;
    Ok(path)
}

impl Perform<Muttrs> for Net {
    fn perform(&mut self, effect: Io) {
        match effect {
            Io::Remote { source, url } => self.remote(source, url),
            Io::Unsubscribe { url, list } => self.unsubscribe(url, list),
            Io::Query { command, typed } => {
                let inbox = self.inbox.clone();
                tokio::task::spawn_blocking(move || {
                    let found = crate::addresses::query(&command, &typed);
                    let _ = inbox.send(Msg::Queried(typed, found));
                });
            }
            Io::SignIn { id, account } => self.sign_in(id, &account),
            Io::Paste { id, address } => {
                if let Some(sign_in) = self.sign_ins.get(&id) {
                    sign_in.paste(address);
                }
            }
            Io::StopSignIn(id) => {
                self.sign_ins.remove(&id);
            }
            Io::Save {
                directory,
                name,
                bytes,
            } => self.send(Msg::Said(
                save(&directory, &name, &bytes)
                    .map(|path| format!("Saved {}", path.display()))
                    .map_err(|error| format!("{error:#}")),
            )),
            Io::OpenFile { name, bytes } => self.send(Msg::Said(
                open_attachment(&name, &bytes)
                    .map(|path| format!("Opened {}", path.display()))
                    .map_err(|error| format!("{error:#}")),
            )),
            #[cfg(feature = "voight")]
            Io::Assess { key, text } => {
                if let Some(assessor) = &self.assessor {
                    assessor.assess(key, text);
                }
            }
            #[cfg(not(feature = "voight"))]
            Io::Assess { .. } => {}
        }
    }

    /// The bytes of a Picture a Pager shows, as it keeps them.
    fn picture(&mut self, source: &str) -> Bytes {
        let bytes = self
            .pictures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(source)
            .cloned();
        Box::pin(async move {
            bytes
                .map(|bytes| bytes.as_ref().clone())
                .ok_or_else(|| "not a Picture of a Message shown".to_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_finds_a_name_of_its_own() {
        let directory = std::env::temp_dir().join(format!("muttrs-save-{}", std::process::id()));
        let first = save(&directory, "a.txt", b"one").unwrap();
        let second = save(&directory, "a.txt", b"two").unwrap();
        assert_eq!(first.file_name().unwrap(), "a.txt");
        assert_eq!(second.file_name().unwrap(), "a (1).txt");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
