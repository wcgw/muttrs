//! Filters: which Messages of the Index `:select` and `:unselect` pick, written as Gmail's search is (`from:ann
//! has:attachment`) and evaluated on what the Index lists (see `docs/adr/0004-filters.md`).

use std::collections::BTreeSet;
use std::fmt;
use std::mem::discriminant;
use std::str::FromStr;

use muttrs_mail::{Address, Envelope};

/// Which Messages a Filter picks: those matching a condition of every group, and none of those negated. The default
/// Filter picks every Message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    /// One group a qualifier written without `-`: a Message must match a condition of each. Groups are never empty,
    /// and each holds one kind of [`Condition`].
    any_of: Vec<Vec<Condition>>,
    /// The qualifiers written with `-`: a Message matching any of them isn't picked.
    none_of: Vec<Condition>,
}

impl Filter {
    /// Whether the Filter picks `envelope`'s Message.
    pub(crate) fn matches(&self, envelope: &Envelope) -> bool {
        self.any_of
            .iter()
            .all(|group| group.iter().any(|condition| condition.matches(envelope)))
            && !self
                .none_of
                .iter()
                .any(|condition| condition.matches(envelope))
    }
}

/// Why a filter doesn't parse, as one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilterError(String);

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Parses a filter: `qualifier:value` terms apart by spaces, each negated by a leading `-`, a value with spaces in
/// double quotes. A qualifier repeated is any of them; different ones, all of them.
impl FromStr for Filter {
    type Err = FilterError;

    fn from_str(filter: &str) -> Result<Self, Self::Err> {
        let mut parsed = Self::default();
        for word in words(filter)? {
            let (negated, term) = match word.strip_prefix('-') {
                Some(term) => (true, term),
                None => (false, word),
            };
            let Some((qualifier, value)) = term.split_once(':') else {
                return Err(FilterError(format!(
                    "expected qualifier:value, like from:ann, not {word}"
                )));
            };
            let value = value.replace('"', "");
            if value.trim().is_empty() {
                return Err(FilterError(format!("{qualifier}: needs a value")));
            }
            let condition = Condition::parse(&qualifier.to_ascii_lowercase(), &value)?;
            if negated {
                parsed.none_of.push(condition);
            } else {
                match parsed
                    .any_of
                    .iter_mut()
                    .find(|group| discriminant(&group[0]) == discriminant(&condition))
                {
                    Some(group) => group.push(condition),
                    None => parsed.any_of.push(vec![condition]),
                }
            }
        }
        Ok(parsed)
    }
}

/// The words of `filter`, apart by spaces outside double quotes.
fn words(filter: &str) -> Result<Vec<&str>, FilterError> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (at, character) in filter.char_indices() {
        if character == '"' {
            quoted = !quoted;
        }
        if character.is_whitespace() && !quoted {
            if let Some(start) = start.take() {
                words.push(&filter[start..at]);
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if quoted {
        return Err(FilterError("a quote isn't closed".to_owned()));
    }
    if let Some(start) = start {
        words.push(&filter[start..]);
    }
    Ok(words)
}

/// The qualifiers muttrs understands, in the order completion offers them: adding one to [`Condition::parse`] means
/// adding it here.
const QUALIFIERS: [&str; 7] = ["from", "to", "cc", "subject", "label", "is", "has"];

/// The `is:` values, in the order completion offers them.
const IS_VALUES: [&str; 5] = ["unread", "read", "flagged", "answered", "draft"];

/// What one `qualifier:value` term asks of a Message. Each variant is a qualifier of its own as far as repeating
/// goes: `is:unread is:flagged` is both, but `is:unread is:read` either.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Condition {
    /// `from:`: a sender's name or address holds this, in lower case.
    From(String),
    /// `to:`: a recipient's in `To`.
    To(String),
    /// `cc:`: a recipient's in `Cc`.
    Cc(String),
    /// `subject:`: the subject holds this, in lower case.
    Subject(String),
    /// `label:`: it has the Label of this name, in lower case.
    Label(String),
    /// `is:unread` (`true`) or `is:read`.
    Unread(bool),
    /// `is:flagged`.
    Flagged,
    /// `is:answered`.
    Answered,
    /// `is:draft`.
    Draft,
    /// `has:attachment`.
    Attachment,
}

impl Condition {
    /// The condition `qualifier:value` stands for, `qualifier` in lower case.
    fn parse(qualifier: &str, value: &str) -> Result<Self, FilterError> {
        let lower = value.to_lowercase();
        match qualifier {
            "from" => Ok(Self::From(lower)),
            "to" => Ok(Self::To(lower)),
            "cc" => Ok(Self::Cc(lower)),
            "subject" => Ok(Self::Subject(lower)),
            "label" => Ok(Self::Label(lower)),
            "is" => match lower.as_str() {
                "unread" => Ok(Self::Unread(true)),
                "read" => Ok(Self::Unread(false)),
                "flagged" => Ok(Self::Flagged),
                "answered" => Ok(Self::Answered),
                "draft" => Ok(Self::Draft),
                "starred" => Err(FilterError(
                    "a starred Message is Flagged: is:flagged".to_owned(),
                )),
                _ => Err(FilterError(format!(
                    "unknown is:{value}; muttrs knows is:{}",
                    IS_VALUES.join(", is:")
                ))),
            },
            "has" if lower == "attachment" => Ok(Self::Attachment),
            "has" => Err(FilterError(format!(
                "unknown has:{value}; muttrs knows has:attachment"
            ))),
            _ => Err(FilterError(format!(
                "unknown qualifier {qualifier}:; muttrs knows {}:",
                QUALIFIERS.join(":, ")
            ))),
        }
    }

    fn matches(&self, envelope: &Envelope) -> bool {
        match self {
            Self::From(text) => holds(&envelope.from, text),
            Self::To(text) => holds(&envelope.to, text),
            Self::Cc(text) => holds(&envelope.cc, text),
            Self::Subject(text) => envelope.subject.to_lowercase().contains(text.as_str()),
            Self::Label(name) => envelope
                .labels
                .iter()
                .any(|label| label.to_lowercase() == *name),
            Self::Unread(unread) => envelope.flags.seen != *unread,
            Self::Flagged => envelope.flags.flagged,
            Self::Answered => envelope.flags.answered,
            Self::Draft => envelope.flags.draft,
            Self::Attachment => envelope.has_attachments,
        }
    }
}

/// Whether the name or address of any of `addresses` holds `text`, in lower case.
fn holds(addresses: &[Address], text: &str) -> bool {
    addresses.iter().any(|address| {
        address.email.to_lowercase().contains(text)
            || address
                .name
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(text))
    })
}

/// The ways the last word of `input` can be completed, each as the whole of `input` with that word completed, in the
/// order to offer them, negated if the word is, ignoring case: while it has no `:`, the qualifiers it starts; once it
/// has, the values of its qualifier its value starts that would pick any of the Messages `listed`. An address's
/// value is started by the address, or by a word of the name; a value with spaces is completed in double quotes.
pub(crate) fn completions(input: &str, listed: &[Envelope]) -> Vec<String> {
    let before = &input[..last_word(input)];
    let word = &input[before.len()..];
    let (negation, term) = match word.strip_prefix('-') {
        Some(term) => ("-", term),
        None => ("", word),
    };
    let Some((qualifier, value)) = term.split_once(':') else {
        let term = term.to_ascii_lowercase();
        return QUALIFIERS
            .iter()
            .filter(|qualifier| qualifier.starts_with(&term))
            .map(|qualifier| format!("{before}{negation}{qualifier}:"))
            .collect();
    };
    let qualifier = qualifier.to_ascii_lowercase();
    let value = value.trim_start_matches('"').to_lowercase();
    values(&qualifier, &value, listed)
        .into_iter()
        .filter(|candidate| {
            Condition::parse(&qualifier, &candidate.replace('"', ""))
                .is_ok_and(|condition| listed.iter().any(|envelope| condition.matches(envelope)))
        })
        .map(|candidate| format!("{before}{negation}{qualifier}:{candidate}"))
        .collect()
}

/// Where the last word of `input` starts: after its last space outside double quotes.
fn last_word(input: &str) -> usize {
    let mut quoted = false;
    let mut start = 0;
    for (at, character) in input.char_indices() {
        if character == '"' {
            quoted = !quoted;
        } else if character.is_whitespace() && !quoted {
            start = at + character.len_utf8();
        }
    }
    start
}

/// The values of `qualifier` that `value` starts, in order: `is:`'s and `has:`'s own, or the addresses or Labels of
/// the Messages `listed`, sorted. Some may pick none of them.
fn values(qualifier: &str, value: &str, listed: &[Envelope]) -> Vec<String> {
    let addresses = |of: fn(&Envelope) -> &[Address]| -> Vec<String> {
        listed
            .iter()
            .flat_map(of)
            .filter(|address| {
                address.email.to_lowercase().starts_with(value)
                    || address.name.as_deref().is_some_and(|name| {
                        name.to_lowercase()
                            .split_whitespace()
                            .any(|word| word.trim_start_matches(['"', '(']).starts_with(value))
                    })
            })
            .map(|address| address.email.to_lowercase())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let own = |values: &[&str]| -> Vec<String> {
        values
            .iter()
            .filter(|candidate| candidate.starts_with(value))
            .map(|candidate| (*candidate).to_owned())
            .collect()
    };
    match qualifier {
        "from" => addresses(|envelope| &envelope.from),
        "to" => addresses(|envelope| &envelope.to),
        "cc" => addresses(|envelope| &envelope.cc),
        "label" => listed
            .iter()
            .flat_map(|envelope| &envelope.labels)
            .filter(|label| label.to_lowercase().starts_with(value))
            .map(|label| (label.to_lowercase(), label))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .map(|label| {
                if label.contains(char::is_whitespace) {
                    format!("\"{label}\"")
                } else {
                    label.clone()
                }
            })
            .collect(),
        "is" => own(&IS_VALUES),
        "has" => own(&["attachment"]),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use muttrs_mail::Flags;

    use super::*;

    fn address(name: Option<&str>, email: &str) -> Address {
        Address {
            name: name.map(str::to_owned),
            email: email.to_owned(),
        }
    }

    fn envelope(uid: u32, from: Address, subject: &str) -> Envelope {
        Envelope {
            uid,
            flags: Flags::default(),
            date: None,
            from: vec![from],
            to: vec![address(Some("Alex"), "alex@example.com")],
            cc: Vec::new(),
            subject: subject.to_owned(),
            message_id: None,
            in_reply_to: Vec::new(),
            references: Vec::new(),
            size: 0,
            has_attachments: false,
            gmail_thread: None,
            labels: Vec::new(),
        }
    }

    fn listed() -> Vec<Envelope> {
        vec![
            Envelope {
                has_attachments: true,
                labels: vec!["GH".into(), "Rust for Linux".into()],
                ..envelope(
                    1,
                    address(Some("Ann Lee"), "ann@example.com"),
                    "Lunch on Thursday?",
                )
            },
            Envelope {
                flags: Flags {
                    seen: true,
                    flagged: true,
                    ..Flags::default()
                },
                cc: vec![address(None, "cy@example.org")],
                labels: vec!["Rustls".into()],
                ..envelope(
                    2,
                    address(Some("Bob"), "bob@example.com"),
                    "The quarterly report",
                )
            },
            Envelope {
                flags: Flags {
                    seen: true,
                    ..Flags::default()
                },
                ..envelope(3, address(Some("Jürgen Müller"), "j@example.de"), "Réunion")
            },
        ]
    }

    /// The UIDs `filter` picks of [`listed`].
    fn picked(filter: &str) -> Vec<u32> {
        let filter: Filter = filter.parse().unwrap();
        listed()
            .iter()
            .filter(|envelope| filter.matches(envelope))
            .map(|envelope| envelope.uid)
            .collect()
    }

    #[test]
    fn qualifiers_pick_by_names_addresses_subjects_and_flags() {
        assert_eq!(picked(""), [1, 2, 3], "no filter, all");
        assert_eq!(picked("from:ann"), [1]);
        assert_eq!(picked("FROM:Example.COM"), [1, 2], "ignoring case");
        assert_eq!(picked("from:müller"), [3], "names too");
        assert_eq!(picked("to:alex cc:cy"), [2]);
        assert_eq!(picked("subject:\"quarterly report\""), [2]);
        assert_eq!(picked("subject:réunion"), [3]);
        assert_eq!(picked("is:unread"), [1]);
        assert_eq!(picked("is:flagged"), [2]);
        assert_eq!(picked("has:attachment"), [1]);
        assert_eq!(picked("label:gh"), [1], "a Label's name, ignoring case");
        assert_eq!(picked("label:\"rust for linux\""), [1]);
        assert_eq!(picked("label:rust"), Vec::<u32>::new(), "the whole name");
        assert_eq!(picked("label:gh label:rustls"), [1, 2]);
        assert_eq!(picked("-label:gh"), [2, 3]);
    }

    #[test]
    fn repeated_is_any_different_is_all_negated_is_none() {
        assert_eq!(picked("from:ann from:bob"), [1, 2]);
        assert_eq!(picked("from:example.com is:read"), [2]);
        assert_eq!(
            picked("is:read is:flagged"),
            [2],
            "two qualifiers, though both is:"
        );
        assert_eq!(picked("is:read is:unread"), [1, 2, 3], "one: read or not");
        assert_eq!(picked("-from:ann"), [2, 3]);
        assert_eq!(picked("-from:ann -is:flagged"), [3], "either leaves it out");
    }

    #[test]
    fn what_it_wont_take_it_says() {
        let problem = |filter: &str| filter.parse::<Filter>().unwrap_err().to_string();
        assert_eq!(
            problem("ann"),
            "expected qualifier:value, like from:ann, not ann"
        );
        assert_eq!(problem("from:"), "from: needs a value");
        assert_eq!(
            problem("list:work"),
            "unknown qualifier list:; muttrs knows from:, to:, cc:, subject:, label:, is:, has:"
        );
        assert_eq!(
            problem("is:starred"),
            "a starred Message is Flagged: is:flagged"
        );
        assert!(problem("is:new").starts_with("unknown is:new; muttrs knows is:unread"));
        assert_eq!(problem("subject:\"lunch"), "a quote isn't closed");
    }

    #[test]
    fn completions_of_qualifiers_then_of_values_that_pick_some() {
        let listed = listed();
        assert_eq!(
            completions("", &listed),
            ["from:", "to:", "cc:", "subject:", "label:", "is:", "has:"]
        );
        assert_eq!(completions("is:read -f", &listed), ["is:read -from:"]);
        assert_eq!(
            completions("from:", &listed),
            [
                "from:ann@example.com",
                "from:bob@example.com",
                "from:j@example.de"
            ]
        );
        assert_eq!(
            completions("from:jür", &listed),
            ["from:j@example.de"],
            "by a word of the name"
        );
        assert_eq!(completions("cc:", &listed), ["cc:cy@example.org"]);
        assert_eq!(
            completions("is:", &listed),
            ["is:unread", "is:read", "is:flagged"],
            "none listed is answered, or a draft"
        );
        assert_eq!(completions("has:", &listed), ["has:attachment"]);
        assert_eq!(completions("subject:lu", &listed), Vec::<String>::new());
        assert_eq!(
            completions("label:", &listed),
            ["label:GH", "label:\"Rust for Linux\"", "label:Rustls"]
        );
        assert_eq!(
            completions("is:read -label:ru", &listed),
            ["is:read -label:\"Rust for Linux\"", "is:read -label:Rustls"]
        );
        assert_eq!(
            completions("label:\"rust f", &listed),
            ["label:\"Rust for Linux\""],
            "a name with spaces, begun in quotes"
        );
    }
}
