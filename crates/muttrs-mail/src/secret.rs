//! Secrets: passwords and refresh tokens, kept in the system keyring (the Secret Service through `secret-tool` on
//! Linux, the login Keychain through `security` on macOS) or printed by a command of the user's choosing. They're
//! never written to a file, and never logged.

use std::fmt;
use std::process::Stdio;

use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// What a secret in the keyring is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An Account's password.
    Password,
    /// An Account's OAuth2 refresh token.
    RefreshToken,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::RefreshToken => "refresh-token",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Password => "password",
            Self::RefreshToken => "OAuth2 refresh token",
        })
    }
}

/// Why a secret couldn't be had, or kept.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// When the keyring has no such secret, or won't give it.
    #[error(
        "there's no {kind} for account {account:?} in the keyring: `muttrs login {account}` stores one"
    )]
    Missing {
        /// The Account's name.
        account: String,
        /// What the secret is for.
        kind: Kind,
    },
    /// When a program can't be run: the keyring's tool, or `sh` for a command.
    #[error("couldn't run {program}: {source} (is it installed?)")]
    Spawn {
        /// The program that couldn't be run.
        program: &'static str,
        /// Why it couldn't be run.
        source: std::io::Error,
    },
    /// When a command, or the keyring's tool, fails: which, and why.
    #[error("{0} failed: {1}")]
    Failed(String, String),
    /// When a command prints nothing.
    #[error("{0} printed nothing")]
    Empty(String),
}

/// The keyring's tool on this platform.
#[cfg(target_os = "macos")]
const TOOL: &str = "security";
#[cfg(not(target_os = "macos"))]
const TOOL: &str = "secret-tool";

/// The secret `kind` of `account` in the keyring.
///
/// # Errors
///
/// If there's none, or the keyring can't be asked.
pub async fn read(account: &str, kind: Kind) -> Result<String, Error> {
    let mut command = Command::new(TOOL);
    if cfg!(target_os = "macos") {
        command
            .args(["find-generic-password", "-s", "muttrs", "-a"])
            .arg(format!("{account}:{}", kind.as_str()))
            .arg("-w");
    } else {
        command.args([
            "lookup",
            "service",
            "muttrs",
            "account",
            account,
            "kind",
            kind.as_str(),
        ]);
    }
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|source| Error::Spawn {
            program: TOOL,
            source,
        })?;
    let secret = first_line(&output.stdout);
    if !output.status.success() || secret.is_empty() {
        return Err(Error::Missing {
            account: account.to_owned(),
            kind,
        });
    }
    Ok(secret)
}

/// Keeps `secret` as the secret `kind` of `account` in the keyring, replacing any there was. The secret goes to the
/// keyring's tool on its standard input, never on its command line.
///
/// # Errors
///
/// If the keyring can't be written.
pub async fn store(account: &str, kind: Kind, secret: &str) -> Result<(), Error> {
    let mut command = Command::new(TOOL);
    let input = if cfg!(target_os = "macos") {
        // `security -i` reads commands from its input; double quotes take backslash escapes.
        let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
        command.arg("-i");
        format!(
            "add-generic-password -U -s muttrs -a {} -w {}\n",
            quote(&format!("{account}:{}", kind.as_str())),
            quote(secret)
        )
    } else {
        command
            .arg("store")
            .arg(format!("--label=muttrs {account} {kind}"))
            .args([
                "service",
                "muttrs",
                "account",
                account,
                "kind",
                kind.as_str(),
            ]);
        secret.to_owned()
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| Error::Spawn {
            program: TOOL,
            source,
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|error| Error::Failed(TOOL.into(), error.to_string()))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|error| Error::Failed(TOOL.into(), error.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Failed(
            TOOL.into(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ))
    }
}

/// The first line `command` prints, run by `sh`: a password from `pass`, `op`, or the like.
///
/// # Errors
///
/// If it fails, or prints nothing.
pub async fn from_command(command: &str) -> Result<String, Error> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|source| Error::Spawn {
            program: "sh",
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Failed(
            format!("`{command}`"),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let secret = first_line(&output.stdout);
    if secret.is_empty() {
        return Err(Error::Empty(format!("`{command}`")));
    }
    Ok(secret)
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end_matches('\r')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commands_give_their_first_line() {
        assert_eq!(
            from_command("printf 'secret\\nmore'").await.unwrap(),
            "secret"
        );
        assert!(matches!(from_command("true").await, Err(Error::Empty(_))));
        assert!(
            matches!(from_command("echo no >&2; exit 3").await, Err(Error::Failed(_, why)) if why == "no")
        );
    }
}
