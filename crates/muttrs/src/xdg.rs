//! Where muttrs keeps its files, after the XDG base directory specification, on macOS as on Linux.

use std::env;
use std::path::PathBuf;

/// The directory of the configuration file: `$XDG_CONFIG_HOME/muttrs`, or `~/.config/muttrs`.
pub(crate) fn config_dir() -> Option<PathBuf> {
    muttrs_dir("XDG_CONFIG_HOME", ".config")
}

/// The directory of what muttrs fetched and keeps: `$XDG_CACHE_HOME/muttrs`, or `~/.cache/muttrs`.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    muttrs_dir("XDG_CACHE_HOME", ".cache")
}

/// The directory of the log: `$XDG_STATE_HOME/muttrs`, or `~/.local/state/muttrs`.
pub(crate) fn state_dir() -> Option<PathBuf> {
    muttrs_dir("XDG_STATE_HOME", ".local/state")
}

/// The user's home directory.
pub(crate) fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `muttrs` under the directory in `variable`, or under `under_home` in the home directory when it isn't set.
fn muttrs_dir(variable: &str, under_home: &str) -> Option<PathBuf> {
    let base = env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(home()?.join(under_home)))?;
    Some(base.join("muttrs"))
}

/// `path` with a leading `~/` meaning the home directory.
pub(crate) fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), home()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}
