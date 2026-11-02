//! What muttrs fetched of an Account, kept on disk so that it shows at once, and offline: its Mailboxes, each
//! Mailbox's Envelopes listed, and the bodies of Messages read, one file a Message, under its Mailbox and the
//! Mailbox's UIDVALIDITY, which renumbering its Messages changes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::envelope::Envelope;
use crate::mailbox::Mailbox;

/// A Mailbox's Envelopes, as last listed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    /// The Mailbox's UIDVALIDITY when they were.
    pub uid_validity: u32,
    /// How many Messages it had.
    pub exists: u32,
    /// The Envelopes listed, oldest first.
    pub envelopes: Vec<Envelope>,
}

/// The bodies of one Account's Messages.
#[derive(Clone, Debug)]
pub struct Cache {
    root: Option<PathBuf>,
}

impl Cache {
    /// Bodies kept under `root`; with `None`, nothing's kept.
    #[must_use]
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root }
    }

    fn path(&self, mailbox: &str, uid_validity: u32, uid: u32) -> Option<PathBuf> {
        Some(
            self.root
                .as_ref()?
                .join(directory_name(mailbox))
                .join(uid_validity.to_string())
                .join(format!("{uid}.eml")),
        )
    }

    /// The Mailboxes as last listed, if they're kept.
    #[must_use]
    pub fn mailboxes(&self) -> Option<Vec<Mailbox>> {
        read_json(&self.root.as_ref()?.join("mailboxes.json"))
    }

    /// Keeps the Mailboxes as listed, as best it can.
    pub fn keep_mailboxes(&self, mailboxes: &[Mailbox]) {
        if let Some(root) = &self.root {
            write_json(&root.join("mailboxes.json"), &mailboxes);
        }
    }

    /// The Envelopes of `mailbox` as last listed, if they're kept.
    #[must_use]
    pub fn envelopes(&self, mailbox: &str) -> Option<Listed> {
        read_json(
            &self
                .root
                .as_ref()?
                .join(directory_name(mailbox))
                .join("envelopes.json"),
        )
    }

    /// Keeps the Envelopes of `mailbox` as listed, as best it can.
    pub fn keep_envelopes(&self, mailbox: &str, listed: &Listed) {
        if let Some(root) = &self.root {
            write_json(
                &root.join(directory_name(mailbox)).join("envelopes.json"),
                listed,
            );
        }
    }

    /// The body of Message `uid` of `mailbox`, if it's kept.
    pub async fn body(&self, mailbox: &str, uid_validity: u32, uid: u32) -> Option<Vec<u8>> {
        tokio::fs::read(self.path(mailbox, uid_validity, uid)?)
            .await
            .ok()
    }

    /// Keeps the body of Message `uid` of `mailbox`, as best it can: a cache that can't be written is no cache.
    pub async fn keep(&self, mailbox: &str, uid_validity: u32, uid: u32, body: &[u8]) {
        let Some(path) = self.path(mailbox, uid_validity, uid) else {
            return;
        };
        let Some(directory) = path.parent() else {
            return;
        };
        if let Err(error) = private_directory(directory).await {
            tracing::debug!(%error, "the body cache can't be written");
            return;
        }
        // Written aside, then renamed, so that a body is never read half written.
        let partial = path.with_extension("partial");
        if tokio::fs::write(&partial, body).await.is_ok() {
            let _ = tokio::fs::rename(&partial, &path).await;
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read(path).ok()?;
    match serde_json::from_slice(&text) {
        Ok(value) => Some(value),
        Err(error) => {
            // Kept by another version of muttrs, say: fetched again.
            tracing::debug!(%error, path = %path.display(), "a cached file can't be read");
            None
        }
    }
}

/// Writes `value` at `path` as JSON, aside then renamed, so that nothing reads it half written.
fn write_json(path: &Path, value: &impl Serialize) {
    let Some(directory) = path.parent() else {
        return;
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    if builder.create(directory).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_vec(value) else {
        return;
    };
    let partial = path.with_extension("partial");
    if std::fs::write(&partial, json).is_ok() {
        let _ = std::fs::rename(&partial, path);
    }
}

/// Creates `directory`, readable by the user only, as mail is private.
async fn private_directory(directory: &Path) -> std::io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(directory).await
}

/// A Mailbox name as a directory name: letters, digits, `-` and `_` as they are, every other byte as `%XX`.
fn directory_name(mailbox: &str) -> String {
    mailbox
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_names_make_safe_directories() {
        assert_eq!(
            directory_name("[Gmail]/Sent Mail"),
            "%5BGmail%5D%2FSent%20Mail"
        );
        assert_eq!(directory_name("../.."), "%2E%2E%2F%2E%2E");
    }

    #[tokio::test]
    async fn bodies_are_kept_by_uid_validity() {
        let root = std::env::temp_dir().join(format!("muttrs-cache-test-{}", std::process::id()));
        let cache = Cache::new(Some(root.clone()));
        assert_eq!(cache.body("INBOX", 1, 7).await, None);
        cache.keep("INBOX", 1, 7, b"Subject: hi\r\n\r\n").await;
        assert_eq!(
            cache.body("INBOX", 1, 7).await.as_deref(),
            Some(&b"Subject: hi\r\n\r\n"[..])
        );
        assert_eq!(cache.body("INBOX", 2, 7).await, None, "renumbered");

        assert_eq!(cache.envelopes("INBOX"), None);
        let listed = Listed {
            uid_validity: 1,
            exists: 2,
            envelopes: vec![crate::envelope::Envelope::read(
                7,
                crate::envelope::Flags::default(),
                None,
                10,
                b"Subject: hi\r\nFrom: Ann <ann@example.com>\r\nDate: Mon, 2 Nov 2026 10:00:00 +0000\r\n\r\n",
            )],
        };
        cache.keep_envelopes("INBOX", &listed);
        assert_eq!(cache.envelopes("INBOX"), Some(listed));
        let mailboxes = vec![Mailbox::listed("INBOX", Some("/"), &[])];
        cache.keep_mailboxes(&mailboxes);
        assert_eq!(cache.mailboxes(), Some(mailboxes));
        let _ = std::fs::remove_dir_all(root);
    }
}
