//! The addresses muttrs has seen, from Envelopes and Messages, to complete recipients with: most often seen first.
//! Kept in a file of one `Name <address>` a line, most often seen first, which an editor can complete from too
//! (vim's and Neovim's `dictionary`).

use std::collections::HashMap;
use std::path::PathBuf;

use muttrs_mail::Address;

/// How many addresses are kept in the file.
const KEPT: usize = 5_000;
/// How many completions are offered.
const OFFERED: usize = 30;

#[derive(Debug, Default)]
pub(crate) struct AddressBook {
    /// By address, in lower case: the address as last seen named, and how often it was seen.
    known: HashMap<String, (Address, u32)>,
    /// Where it's kept.
    path: Option<PathBuf>,
    /// Whether it learnt anything since it was kept.
    changed: bool,
}

impl AddressBook {
    /// The addresses kept at `path`, if there are any.
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let mut book = Self {
            path,
            ..Self::default()
        };
        let Some(text) = book
            .path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return book;
        };
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        // The first is the most often seen: seen as often as there are after it, plus one.
        for (rank, line) in lines.iter().enumerate() {
            let Ok(addresses) = muttrs_mail::compose::parse_addresses(line) else {
                continue;
            };
            for address in addresses {
                let seen = u32::try_from(lines.len() - rank).unwrap_or(u32::MAX);
                book.known
                    .entry(address.email.to_ascii_lowercase())
                    .or_insert((address, seen));
            }
        }
        book
    }

    /// Notes that `addresses` were seen.
    pub(crate) fn learn<'a>(&mut self, addresses: impl IntoIterator<Item = &'a Address>) {
        for address in addresses {
            if !address.email.contains('@') {
                continue;
            }
            self.changed = true;
            let named = address
                .name
                .as_deref()
                .is_some_and(|name| !name.trim().is_empty());
            let entry = self
                .known
                .entry(address.email.to_ascii_lowercase())
                .or_insert_with(|| (address.clone(), 0));
            entry.1 = entry.1.saturating_add(1);
            if named {
                entry.0.name.clone_from(&address.name);
            }
        }
    }

    /// The addresses whose name or address holds `fragment`, ignoring case, those starting so first, then the most
    /// often seen, as headers write them.
    pub(crate) fn complete(&self, fragment: &str) -> Vec<String> {
        let fragment = fragment.trim().to_lowercase();
        if fragment.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(bool, u32, String)> = self
            .known
            .values()
            .filter_map(|(address, seen)| {
                let written = address.to_string();
                let lower = written.to_lowercase();
                lower.contains(&fragment).then(|| {
                    let starts = lower.starts_with(&fragment)
                        || address.email.to_lowercase().starts_with(&fragment)
                        || lower
                            .split_whitespace()
                            .any(|word| word.trim_start_matches(['"', '<']).starts_with(&fragment));
                    (starts, *seen, written)
                })
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        found
            .into_iter()
            .take(OFFERED)
            .map(|(_, _, written)| written)
            .collect()
    }

    /// Keeps the addresses in the file, the most often seen first, if anything was learnt.
    pub(crate) fn save(&mut self) {
        let Some(path) = &self.path else { return };
        if !self.changed {
            return;
        }
        let mut known: Vec<&(Address, u32)> = self.known.values().collect();
        known.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.email.cmp(&b.0.email)));
        let text = known
            .iter()
            .take(KEPT)
            .fold(String::new(), |mut text, (address, _)| {
                text.push_str(&address.to_string());
                text.push('\n');
                text
            });
        if let Some(directory) = path.parent() {
            let _ = std::fs::create_dir_all(directory);
        }
        if std::fs::write(path, text).is_ok() {
            self.changed = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::to_owned),
            email: email.to_owned(),
        }
    }

    #[test]
    fn completes_by_name_or_address_starting_then_most_seen() {
        let mut book = AddressBook::default();
        book.learn(&[
            address(Some("Ann Lee"), "ann@example.com"),
            address(None, "joann@example.com"),
        ]);
        book.learn(&[
            address(None, "joann@example.com"),
            address(Some("Bob"), "bob@ann.example"),
        ]);
        assert_eq!(
            book.complete("ann"),
            [
                "Ann Lee <ann@example.com>",
                "joann@example.com",
                "Bob <bob@ann.example>"
            ],
            "Ann starts a name; the others only hold it, joann seen more"
        );
        assert_eq!(book.complete("LEE"), ["Ann Lee <ann@example.com>"]);
        assert_eq!(book.complete(" "), Vec::<String>::new());
    }

    #[test]
    fn names_seen_later_win_and_the_file_keeps_the_order() {
        let path = std::env::temp_dir().join(format!("muttrs-addresses-{}", std::process::id()));
        let mut book = AddressBook::load(Some(path.clone()));
        book.learn(&[
            address(None, "ann@example.com"),
            address(Some("Bob"), "bob@example.com"),
        ]);
        book.learn(&[
            address(Some("Ann"), "ANN@example.com"),
            address(None, "bob@example.com"),
        ]);
        book.learn(&[address(None, "cy@example.com")]);
        book.learn(&[address(None, "bob@example.com")]);
        book.save();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Bob <bob@example.com>\nAnn <ann@example.com>\ncy@example.com\n"
        );
        let loaded = AddressBook::load(Some(path.clone()));
        assert_eq!(
            loaded.complete("example"),
            [
                "Bob <bob@example.com>",
                "Ann <ann@example.com>",
                "cy@example.com"
            ]
        );
        std::fs::remove_file(path).unwrap();
    }
}
