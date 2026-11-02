//! The bodies of Messages fetched, kept on disk so that opening one again needs no server: one file a Message,
//! under its Mailbox and the Mailbox's UIDVALIDITY, which renumbering its Messages changes.

use std::path::{Path, PathBuf};

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
        let _ = std::fs::remove_dir_all(root);
    }
}
