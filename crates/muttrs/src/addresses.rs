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
    /// The addresses of the Accounts' address books (CardDAV), by address in lower case: offered too, never kept.
    contacts: HashMap<String, Address>,
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
        // Contacts never seen are offered after those that were, as seen never.
        let contacts = self
            .contacts
            .iter()
            .filter(|(email, _)| !self.known.contains_key(*email))
            .map(|(_, address)| (address, 0));
        let mut found: Vec<(bool, u32, String)> = self
            .known
            .values()
            .map(|(address, seen)| (address, *seen))
            .chain(contacts)
            .filter_map(|(address, seen)| {
                let written = address.to_string();
                let lower = written.to_lowercase();
                lower.contains(&fragment).then(|| {
                    let starts = lower.starts_with(&fragment)
                        || address.email.to_lowercase().starts_with(&fragment)
                        || lower
                            .split_whitespace()
                            .any(|word| word.trim_start_matches(['"', '<']).starts_with(&fragment));
                    (starts, seen, written)
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

    /// Offers the addresses of an address book's `contacts` too, under their names there.
    pub(crate) fn add_contacts(&mut self, contacts: Vec<Address>) {
        for contact in contacts {
            self.contacts
                .insert(contact.email.to_ascii_lowercase(), contact);
        }
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

/// The addresses `command` finds for `fragment`, as mutt's `query_command` gives them: `%s` in it is `fragment`,
/// quoted for the shell (else it goes at the end); it prints a line of its own first, then one an address,
/// `address<TAB>name<TAB>anything else`. Waits for it: to be run aside.
pub(crate) fn query(command: &str, fragment: &str) -> Vec<Address> {
    let quoted = format!("'{}'", fragment.replace('\'', "'\\''"));
    let command = if command.contains("%s") {
        command.replace("%s", &quoted)
    } else {
        format!("{command} {quoted}")
    };
    match std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(output) => queried(&String::from_utf8_lossy(&output.stdout)),
        Err(error) => {
            tracing::warn!(command, %error, "the query command couldn't run");
            Vec::new()
        }
    }
}

/// The addresses a query command printed, after its first line.
fn queried(output: &str) -> Vec<Address> {
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let email = fields.next()?.trim();
            let name = fields.next().map(str::trim).filter(|name| !name.is_empty());
            email.contains('@').then(|| Address {
                name: name.map(str::to_owned),
                email: email.to_owned(),
            })
        })
        .collect()
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

    #[test]
    fn contacts_are_offered_after_what_was_seen_and_never_kept() {
        let path = std::env::temp_dir().join(format!("muttrs-contacts-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut book = AddressBook::load(Some(path.clone()));
        book.learn(&[address(Some("Ann Seen"), "ann@example.com")]);
        book.add_contacts(vec![
            address(Some("Ann Contact"), "ann@example.com"),
            address(Some("Anna Book"), "anna@example.com"),
        ]);
        assert_eq!(
            book.complete("ann"),
            ["Ann Seen <ann@example.com>", "Anna Book <anna@example.com>"]
        );
        book.save();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "Ann Seen <ann@example.com>\n"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_query_command_is_read_as_mutt_reads_it() {
        assert_eq!(
            queried(
                "Searching...\nann@example.com\tAnn Example\twork\nbob@example.com\t\nnot an address\n"
            ),
            [
                address(Some("Ann Example"), "ann@example.com"),
                address(None, "bob@example.com")
            ]
        );
        assert_eq!(
            query("echo first; echo %s@example.com", "o'neil")
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["o'neil@example.com"],
            "the fragment quoted for the shell"
        );
    }
}
