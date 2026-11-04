//! The History: what was typed at the prompt before, to go back to, as in vim: commands (`:`), and searches (`/` and
//! `S`, one History for both). Kept in a file of one entry a line, oldest first, each after what typed it (`:` or `/`),
//! read again before an entry is added, so that two muttrs running keep what each other typed.

use std::path::PathBuf;

/// How many entries of each kind are kept.
const KEPT: usize = 200;

/// What a History entry was typed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Command,
    /// A search, of what's listed or shown (`/`), or of the server (`S`).
    Search,
    /// A shell command, run (`!`) or piped to (`|`).
    Shell,
}

impl Kind {
    /// What starts its entries' lines in the file.
    fn lead(self) -> char {
        match self {
            Self::Command => ':',
            Self::Search => '/',
            Self::Shell => '!',
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct History {
    /// Oldest first, each once.
    commands: Vec<String>,
    searches: Vec<String>,
    shells: Vec<String>,
    /// Where it's kept.
    path: Option<PathBuf>,
}

impl History {
    /// The History kept at `path`, if there is one.
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let mut history = Self {
            path,
            ..Self::default()
        };
        history.read();
        history
    }

    /// The entries of `kind`, oldest first.
    pub(crate) fn entries(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Command => &self.commands,
            Kind::Search => &self.searches,
            Kind::Shell => &self.shells,
        }
    }

    /// Adds `entry` as the newest of `kind` (an entry typed again moves), to what the file has now, and keeps it
    /// there. A blank one isn't kept.
    pub(crate) fn add(&mut self, kind: Kind, entry: &str) {
        if entry.trim().is_empty() || entry.contains(['\n', '\r']) {
            return;
        }
        self.read();
        self.push(kind, entry.to_owned());
        self.write();
    }

    fn push(&mut self, kind: Kind, entry: String) {
        let entries = match kind {
            Kind::Command => &mut self.commands,
            Kind::Search => &mut self.searches,
            Kind::Shell => &mut self.shells,
        };
        entries.retain(|kept| *kept != entry);
        entries.push(entry);
        if entries.len() > KEPT {
            entries.drain(..entries.len() - KEPT);
        }
    }

    /// Takes what the file has, if it can be read.
    fn read(&mut self) {
        let Some(text) = self
            .path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return;
        };
        self.commands.clear();
        self.searches.clear();
        self.shells.clear();
        for line in text.lines() {
            let mut characters = line.chars();
            let kind = match characters.next() {
                Some(':') => Kind::Command,
                Some('/') => Kind::Search,
                Some('!') => Kind::Shell,
                _ => continue,
            };
            let entry = characters.as_str();
            if !entry.trim().is_empty() {
                self.push(kind, entry.to_owned());
            }
        }
    }

    /// Writes it to the file, aside then renamed so that nothing reads it half written; readable by the user alone,
    /// as searches can say what's in their mail.
    fn write(&self) {
        let Some(path) = &self.path else { return };
        if let Some(directory) = path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        let mut text = String::new();
        for kind in [Kind::Command, Kind::Search, Kind::Shell] {
            for entry in self.entries(kind) {
                text.push(kind.lead());
                text.push_str(entry);
                text.push('\n');
            }
        }
        let partial = path.with_extension("partial");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let written = options
            .open(&partial)
            .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()));
        if written.is_ok() {
            let _ = std::fs::rename(&partial, path);
        }
    }
}

/// Going back through a History's entries from what's typed, as `Up` and `Down` do (those that start with it), or
/// `Ctrl-P` and `Ctrl-N` (all of them).
#[derive(Debug)]
pub(crate) struct Recall {
    /// What was typed before going back: shown again past the newest.
    typed: String,
    /// The entry shown, by its place among the entries; None for what was typed.
    at: Option<usize>,
}

impl Recall {
    pub(crate) fn new(typed: String) -> Self {
        Self { typed, at: None }
    }

    /// Shows the entry before the one shown among `entries` (oldest first), if `older`, else the one after: the
    /// next that starts with what was typed if `matching`, else the next; what was typed, past the newest. None if
    /// there's none, the one shown staying.
    pub(crate) fn step<'a>(
        &'a mut self,
        entries: &'a [String],
        older: bool,
        matching: bool,
    ) -> Option<&'a str> {
        if self.at.is_some_and(|at| at >= entries.len()) {
            self.at = None;
        }
        let shown = self.at.map_or(self.typed.as_str(), |at| &entries[at]);
        let wanted =
            |entry: &String| *entry != shown && (!matching || entry.starts_with(&self.typed));
        if older {
            let before = self.at.unwrap_or(entries.len());
            self.at = Some(entries[..before].iter().rposition(wanted)?);
        } else {
            let after = self.at? + 1;
            self.at = entries[after..]
                .iter()
                .position(wanted)
                .map(|found| after + found);
        }
        Some(self.at.map_or(self.typed.as_str(), |at| &entries[at]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|&entry| entry.to_owned()).collect()
    }

    #[test]
    fn up_and_down_go_through_what_starts_as_typed() {
        let entries = lines(&["cd Inbox", "select from:ann", "refresh", "select is:unread"]);
        let mut recall = Recall::new("sel".into());
        assert_eq!(recall.step(&entries, true, true), Some("select is:unread"));
        assert_eq!(recall.step(&entries, true, true), Some("select from:ann"));
        assert_eq!(recall.step(&entries, true, true), None, "none older");
        assert_eq!(recall.step(&entries, false, true), Some("select is:unread"));
        assert_eq!(recall.step(&entries, false, true), Some("sel"), "as typed");
        assert_eq!(recall.step(&entries, false, true), None, "none newer");

        let mut recall = Recall::new("sel".into());
        assert_eq!(recall.step(&entries, true, false), Some("select is:unread"));
        assert_eq!(
            recall.step(&entries, true, false),
            Some("refresh"),
            "Ctrl-P: any"
        );
        assert_eq!(recall.step(&[], true, true), None, "an empty History");
    }

    #[test]
    fn what_is_typed_already_is_passed_over() {
        let entries = lines(&["cd Inbox/Work", "cd Inbox"]);
        let mut recall = Recall::new("cd Inbox".into());
        assert_eq!(recall.step(&entries, true, true), Some("cd Inbox/Work"));
    }

    #[test]
    fn entries_are_kept_once_the_newest_last_with_what_another_muttrs_kept() {
        let path = std::env::temp_dir().join(format!("muttrs-history-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut history = History::load(Some(path.clone()));
        history.add(Kind::Command, "select is:unread");
        history.add(Kind::Search, "lunch");
        history.add(Kind::Command, "cd Inbox");
        history.add(Kind::Command, "select is:unread");
        history.add(Kind::Command, "  ");

        let mut other = History::load(Some(path.clone()));
        other.add(Kind::Search, "/tmp: full");
        history.add(Kind::Search, "rust");
        assert_eq!(
            history.entries(Kind::Command),
            ["cd Inbox", "select is:unread"]
        );
        assert_eq!(
            history.entries(Kind::Search),
            ["lunch", "/tmp: full", "rust"]
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            ":cd Inbox\n:select is:unread\n/lunch\n//tmp: full\n/rust\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn only_the_newest_are_kept() {
        let mut history = History::default();
        for n in 0..KEPT + 5 {
            history.add(Kind::Search, &n.to_string());
        }
        let searches = history.entries(Kind::Search);
        assert_eq!(searches.len(), KEPT);
        assert_eq!(searches.first().map(String::as_str), Some("5"));
    }
}
