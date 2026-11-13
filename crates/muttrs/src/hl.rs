//! muttrs' highlight groups (wcgw-kit's D7): each linked to a standard group, so any colorscheme colours it, with
//! muttrs' own colours for tokyonight-moon, as its built-in theme had them, and for `default`, the terminal's own.

pub(crate) use wcgw_kit::hl::{Group, Theme};

/// Secondary text: dates, header names, rules, hints, a thread's tree.
pub(crate) const DIM: Group = Group::new("MuttrsDim");
/// What wants attention without alarm: the Unread mark, counts, the Selection's mark, an Account's name.
pub(crate) const ACCENT: Group = Group::new("MuttrsAccent");
/// The Index's row under the cursor.
pub(crate) const CURSOR_LINE: Group = Group::new("MuttrsCursorLine");
/// The open Mailbox, in the sidebar.
pub(crate) const OPEN: Group = Group::new("MuttrsOpen");
/// Unread Messages, and Mailboxes with Unread Messages.
pub(crate) const UNREAD: Group = Group::new("MuttrsUnread");
/// Read Messages, and Mailboxes without Unread ones.
pub(crate) const READ: Group = Group::new("MuttrsRead");
/// A Flagged Message's mark.
pub(crate) const FLAGGED: Group = Group::new("MuttrsFlagged");
/// A Label a Message has, in the Index: its name, padded a cell each side.
pub(crate) const LABEL: Group = Group::new("MuttrsLabel");
/// A Message's subject, in the Pager and a Draft.
pub(crate) const SUBJECT: Group = Group::new("MuttrsSubject");
/// Attachments, and their mark.
pub(crate) const ATTACHMENT: Group = Group::new("MuttrsAttachment");
/// The line saying Remote Content is blocked.
pub(crate) const BANNER: Group = Group::new("MuttrsBanner");
/// What's wrong: an Account offline, a Draft with nobody to send to.
pub(crate) const ERROR: Group = Group::new("MuttrsError");
/// The background authors' colours are judged against, kept only where they read on it: the terminal's, which
/// muttrs can't ask. Without one, bodies are drawn in the colorscheme's colours alone.
pub(crate) const PAGE: Group = Group::new("MuttrsPage");
/// In voight's Heatmap, prose voight scored 0%.
pub(crate) const HEAT_HUMAN: Group = Group::new("VoightHeatHuman");
/// Prose voight scored 50%.
pub(crate) const HEAT_EVEN: Group = Group::new("VoightHeatEven");
/// Prose voight scored 100%.
pub(crate) const HEAT_LLM: Group = Group::new("VoightHeatLlm");

/// muttrs' groups, and the standard ones they're drawn as when a colorscheme says nothing of them.
const GROUPS: [(Group, Group); 16] = [
    (DIM, Group::COMMENT),
    (ACCENT, Group::FUNCTION),
    (CURSOR_LINE, Group::VISUAL),
    (OPEN, Group::CURSOR_LINE),
    (UNREAD, Group::BOLD),
    (READ, Group::NORMAL),
    (FLAGGED, Group::DIAGNOSTIC_WARN),
    (LABEL, Group::SPECIAL),
    (SUBJECT, Group::TITLE),
    (ATTACHMENT, Group::SPECIAL),
    (BANNER, Group::DIAGNOSTIC_WARN),
    (ERROR, Group::DIAGNOSTIC_ERROR),
    (PAGE, Group::NORMAL),
    (HEAT_HUMAN, Group::DIFF_ADD),
    (HEAT_EVEN, Group::DIFF_CHANGE),
    (HEAT_LLM, Group::DIFF_DELETE),
];

/// muttrs' colours for the colorschemes it had themes for: tokyonight-moon's as its built-in theme drew them, and
/// the terminal's own (`default`, which was the `terminal` theme). The other tokyonights' backgrounds, for authors'
/// colours.
const OVERRIDES: [(&str, &str); 6] = [
    (
        "tokyonight-moon",
        "\
         hi MuttrsDim guifg=#636da6\n\
         hi MuttrsAccent guifg=#82aaff\n\
         hi MuttrsCursorLine guibg=#2d3f76\n\
         hi MuttrsOpen guibg=#2f334d\n\
         hi MuttrsUnread gui=bold guifg=#c8d3f5\n\
         hi MuttrsRead guifg=#828bb8\n\
         hi MuttrsFlagged guifg=#ff966c\n\
         hi MuttrsLabel guifg=#86e1fc guibg=#2f334d\n\
         hi MuttrsSubject gui=bold guifg=#c8d3f5\n\
         hi MuttrsAttachment guifg=#4fd6be\n\
         hi MuttrsBanner guifg=#ffc777\n\
         hi MuttrsError guifg=#ff757f\n\
         hi MuttrsPage guibg=#222436\n\
         hi VoightHeatHuman guifg=#222436 guibg=#c3e88d\n\
         hi VoightHeatEven guifg=#222436 guibg=#ffc777\n\
         hi VoightHeatLlm guifg=#222436 guibg=#ff757f\n\
",
    ),
    ("tokyonight-night", "hi MuttrsPage guibg=#1a1b26\n"),
    ("tokyonight", "hi MuttrsPage guibg=#1a1b26\n"),
    ("tokyonight-storm", "hi MuttrsPage guibg=#24283b\n"),
    ("tokyonight-day", "hi MuttrsPage guibg=#e1e2e7\n"),
    (
        "default",
        "\
         hi MuttrsDim guifg=darkgray\n\
         hi MuttrsAccent gui=bold\n\
         hi MuttrsCursorLine gui=reverse\n\
         hi MuttrsOpen gui=underline\n\
         hi MuttrsUnread gui=bold\n\
         hi MuttrsRead NONE\n\
         hi MuttrsFlagged guifg=yellow\n\
         hi MuttrsLabel gui=reverse guifg=cyan\n\
         hi MuttrsSubject gui=bold\n\
         hi MuttrsAttachment guifg=cyan\n\
         hi MuttrsBanner guifg=yellow\n\
         hi MuttrsError guifg=red\n\
         hi MuttrsPage NONE\n\
         hi VoightHeatHuman guifg=black guibg=green\n\
         hi VoightHeatEven guifg=black guibg=yellow\n\
         hi VoightHeatLlm guifg=black guibg=red\n\
",
    ),
];

/// The kit's theme with muttrs' groups and colours.
pub(crate) fn theme() -> Theme {
    let mut theme = Theme::new();
    for (group, link) in GROUPS {
        theme.define(group, Some(link));
    }
    for (scheme, text) in OVERRIDES {
        theme
            .add_overrides(scheme, text)
            .expect("muttrs' colours name muttrs' groups");
    }
    theme
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier};

    use super::*;

    #[test]
    fn tokyonight_moon_draws_what_the_built_in_theme_drew() {
        let theme = theme();
        assert_eq!(theme.name(), "tokyonight-moon");
        assert_eq!(theme.style(READ).fg, Some(Color::Rgb(0x82, 0x8b, 0xb8)));
        assert_eq!(
            theme.style(CURSOR_LINE).bg,
            Some(Color::Rgb(0x2d, 0x3f, 0x76))
        );
        assert!(theme.style(UNREAD).add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.style(PAGE).bg, Some(Color::Rgb(0x22, 0x24, 0x36)));
    }

    #[test]
    fn every_colorscheme_colours_muttrs_groups_through_their_links() {
        let mut theme = theme();
        for scheme in theme.colorscheme_names() {
            theme.colorscheme(&scheme).expect("a built-in colorscheme");
        }
        theme.colorscheme("tokyonight-storm").expect("storm");
        assert_eq!(theme.style(DIM).fg, theme.style(Group::COMMENT).fg);
        theme.colorscheme("default").expect("default");
        assert_eq!(theme.style(FLAGGED).fg, Some(Color::Yellow));
    }
}
