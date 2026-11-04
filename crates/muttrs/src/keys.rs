//! Keys of one's own: the configuration's `[keys.index]`, `[keys.sidebar]`, `[keys.pager]` and `[keys.draft]` map a
//! key to the keys it types in its place, as vim's `noremap` does: one, to give a key muttrs has another key (mutt's
//! `bind`); several, typed in turn, as a macro (mutt's `macro`): `"<C-a>" = ":cd Archive<Enter>"`. What a key maps to
//! isn't mapped again.
//!
//! Keys are written as vim writes them: a character as itself, and `<Enter>`, `<Esc>`, `<Tab>`, `<S-Tab>`,
//! `<Space>`, `<BS>`, `<Del>`, `<Up>`, `<Down>`, `<Left>`, `<Right>`, `<Home>`, `<End>`, `<PageUp>`, `<PageDown>`,
//! `<F1>` to `<F12>`, `<lt>` (for `<`); with Control as `<C-x>` and Alt as `<M-x>` (or `<A-x>`).

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

/// Where a key's pressed, as the keys of one's own are mapped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Context {
    Index,
    Sidebar,
    Pager,
    /// A Draft shown, before it's sent.
    Draft,
}

/// The keys of one's own, by where they're pressed.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Keymap {
    index: Mapped,
    sidebar: Mapped,
    pager: Mapped,
    draft: Mapped,
}

impl Keymap {
    /// The keys `key`, pressed in `context`, types in its place; None if it's not one of one's own there.
    pub(crate) fn typed(&self, context: Context, key: &KeyEvent) -> Option<&[KeyEvent]> {
        let mapped = match context {
            Context::Index => &self.index,
            Context::Sidebar => &self.sidebar,
            Context::Pager => &self.pager,
            Context::Draft => &self.draft,
        };
        mapped
            .keys
            .get(&Key::of(key))
            .map(|keys| keys.typed.as_slice())
    }

    /// The keys of one's own in `context`, as written, by the key: for the help.
    pub(crate) fn written(&self, context: Context) -> Vec<(&str, &str)> {
        let mapped = match context {
            Context::Index => &self.index,
            Context::Sidebar => &self.sidebar,
            Context::Pager => &self.pager,
            Context::Draft => &self.draft,
        };
        let mut written: Vec<(&str, &str)> = mapped
            .keys
            .values()
            .map(|keys| (keys.key.as_str(), keys.written.as_str()))
            .collect();
        written.sort_unstable();
        written
    }
}

/// The keys of one's own of one context.
#[derive(Clone, Debug, Default)]
struct Mapped {
    keys: HashMap<Key, Keys>,
}

/// What a key of one's own types.
#[derive(Clone, Debug)]
struct Keys {
    /// The key, as written.
    key: String,
    /// What it types, as written, and read.
    written: String,
    typed: Vec<KeyEvent>,
}

impl<'de> Deserialize<'de> for Mapped {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let written = HashMap::<String, String>::deserialize(deserializer)?;
        let mut keys = HashMap::new();
        for (key, typed) in written {
            let [pressed] = read(&key).map_err(serde::de::Error::custom)?[..] else {
                return Err(serde::de::Error::custom(format!(
                    "{key:?} is more than one key: a key of one's own is one key, which types others"
                )));
            };
            let read_typed = read(&typed).map_err(serde::de::Error::custom)?;
            if read_typed.is_empty() {
                return Err(serde::de::Error::custom(format!("{key:?} types nothing")));
            }
            keys.insert(
                Key::of(&pressed),
                Keys {
                    key,
                    written: typed,
                    typed: read_typed,
                },
            );
        }
        Ok(Self { keys })
    }
}

/// A key as it's told from others: Shift is in the character it types, as `J`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    code: KeyCode,
    control: bool,
    alt: bool,
}

impl Key {
    fn of(event: &KeyEvent) -> Self {
        Self {
            code: event.code,
            control: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
        }
    }
}

/// The keys `text` writes, in order.
fn read(text: &str) -> Result<Vec<KeyEvent>, String> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(character) = rest.chars().next() {
        let named = (character == '<')
            .then(|| rest.find('>').map(|end| &rest[1..end]))
            .flatten()
            .filter(|name| !name.is_empty());
        if let Some(name) = named {
            keys.push(named_key(name).ok_or_else(|| format!("<{name}> is no key muttrs knows"))?);
            rest = &rest[name.len() + 2..];
        } else {
            keys.push(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
            rest = &rest[character.len_utf8()..];
        }
    }
    Ok(keys)
}

/// The key `<name>` writes.
fn named_key(name: &str) -> Option<KeyEvent> {
    let lower = name.to_ascii_lowercase();
    if lower == "s-tab" {
        return Some(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    // The prefixes are ASCII: what follows them starts two bytes in.
    let (modifiers, base) = if lower.starts_with("c-") {
        (KeyModifiers::CONTROL, &name[2..])
    } else if lower.starts_with("m-") || lower.starts_with("a-") {
        (KeyModifiers::ALT, &name[2..])
    } else {
        (KeyModifiers::NONE, name)
    };
    let mut characters = base.chars();
    let code = match (characters.next(), characters.next()) {
        // `<C-x>`: the key as it's typed, a letter in lower case as Control sends it.
        (Some(character), None) if !modifiers.is_empty() => {
            KeyCode::Char(if modifiers == KeyModifiers::CONTROL {
                character.to_ascii_lowercase()
            } else {
                character
            })
        }
        _ => match base.to_ascii_lowercase().as_str() {
            "enter" | "cr" | "return" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "space" => KeyCode::Char(' '),
            "bs" | "backspace" => KeyCode::Backspace,
            "del" | "delete" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "lt" => KeyCode::Char('<'),
            function => KeyCode::F(
                function
                    .strip_prefix('f')?
                    .parse()
                    .ok()
                    .filter(|n| (1..=12).contains(n))?,
            ),
        },
    };
    Some(KeyEvent::new(code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn keys_are_written_as_vim_writes_them() {
        assert_eq!(
            read(":cd Archive<Enter>").unwrap(),
            [
                key(KeyCode::Char(':')),
                key(KeyCode::Char('c')),
                key(KeyCode::Char('d')),
                key(KeyCode::Char(' ')),
                key(KeyCode::Char('A')),
                key(KeyCode::Char('r')),
                key(KeyCode::Char('c')),
                key(KeyCode::Char('h')),
                key(KeyCode::Char('i')),
                key(KeyCode::Char('v')),
                key(KeyCode::Char('e')),
                key(KeyCode::Enter),
            ]
        );
        assert_eq!(
            read("<C-X><m-j><S-Tab><lt><Space><F5>").unwrap(),
            [
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT),
                KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
                key(KeyCode::Char('<')),
                key(KeyCode::Char(' ')),
                key(KeyCode::F(5)),
            ]
        );
        assert_eq!(read("a<b").unwrap().len(), 3, "a < alone is itself");
        assert_eq!(read("<Nope>").unwrap_err(), "<Nope> is no key muttrs knows");
    }

    #[test]
    fn a_key_of_ones_own_types_others_where_it_is_mapped() {
        let keymap: Keymap = toml::from_str(
            "[index]\nx = \"a\"\n\"<C-a>\" = \":cd Archive<Enter>\"\n[pager]\n\"<Space>\" = \"J\"",
        )
        .unwrap();
        let typed = |context, pressed: KeyEvent| keymap.typed(context, &pressed).map(<[_]>::len);
        assert_eq!(typed(Context::Index, key(KeyCode::Char('x'))), Some(1));
        assert_eq!(
            typed(
                Context::Index,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
            ),
            Some(12)
        );
        assert_eq!(
            typed(Context::Pager, key(KeyCode::Char('x'))),
            None,
            "the Index's"
        );
        assert_eq!(typed(Context::Pager, key(KeyCode::Char(' '))), Some(1));
        assert_eq!(
            typed(
                Context::Index,
                KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT)
            ),
            None,
            "x isn't X"
        );
        assert_eq!(
            keymap.written(Context::Index),
            [("<C-a>", ":cd Archive<Enter>"), ("x", "a")]
        );
    }

    #[test]
    fn a_key_of_ones_own_is_one_key_and_types_some() {
        let error = |text: &str| toml::from_str::<Keymap>(text).unwrap_err().to_string();
        assert!(error("[index]\ngi = \":cd Inbox<Enter>\"").contains("more than one key"));
        assert!(error("[index]\nx = \"\"").contains("types nothing"));
        assert!(error("[index]\nx = \"<Bogus>\"").contains("<Bogus> is no key"));
        assert!(error("[inbox]\nx = \"a\"").contains("unknown field"));
    }
}
