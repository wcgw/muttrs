//! muttrs' icons (wcgw-kit's D9): Nerd Font glyphs (Octicons and Material Design, as named in Nerd Fonts'
//! `glyphnames.json`), each with a plain fallback, both drawn two cells wide by the kit. `:set icons=` picks, `auto`
//! by the terminal. The kit has its own for loading, links and pictures.

use crate::app::Kit;

pub(crate) const UNREAD: &str = "MuttrsUnread";
pub(crate) const FLAGGED: &str = "MuttrsFlagged";
pub(crate) const ANSWERED: &str = "MuttrsAnswered";
pub(crate) const ATTACHMENT: &str = "MuttrsAttachment";
pub(crate) const BLOCKED: &str = "MuttrsBlocked";
pub(crate) const INBOX: &str = "MuttrsInbox";
pub(crate) const SENT: &str = "MuttrsSent";
pub(crate) const DRAFTS: &str = "MuttrsDrafts";
pub(crate) const TRASH: &str = "MuttrsTrash";
pub(crate) const JUNK: &str = "MuttrsJunk";
pub(crate) const ARCHIVE: &str = "MuttrsArchive";
pub(crate) const ALL: &str = "MuttrsAll";
pub(crate) const FOLDER: &str = "MuttrsFolder";
pub(crate) const ACCOUNT: &str = "MuttrsAccount";
pub(crate) const ALERT: &str = "MuttrsAlert";
pub(crate) const ONLINE: &str = "MuttrsOnline";
pub(crate) const OFFLINE: &str = "MuttrsOffline";
/// The kit's: waiting on the server.
pub(crate) const LOADING: &str = "loading";
/// The kit's: an image not drawn, before its description.
pub(crate) const PICTURE: &str = "picture";

/// Each icon: its name, its Nerd Font glyph, and its plain fallback (one cell, or none).
const ICONS: [(&str, &str, &str); 17] = [
    (UNREAD, "\u{f444}", "●"),     // oct-dot_fill
    (FLAGGED, "\u{f51f}", "★"),    // oct-star_fill
    (ANSWERED, "\u{f4a8}", "↩"),   // oct-reply
    (ATTACHMENT, "\u{f4fb}", "+"), // oct-paperclip
    (BLOCKED, "\u{f511}", "⊘"),    // oct-shield_lock
    (INBOX, "\u{f48d}", ""),       // oct-inbox
    (SENT, "\u{f4fa}", ""),        // oct-paper_airplane
    (DRAFTS, "\u{f448}", ""),      // oct-pencil
    (TRASH, "\u{f48e}", ""),       // oct-trash
    (JUNK, "\u{f421}", ""),        // oct-alert
    (ARCHIVE, "\u{f411}", ""),     // oct-archive
    (ALL, "\u{f51e}", ""),         // oct-stack
    (FOLDER, "\u{f413}", ""),      // oct-file_directory
    (ACCOUNT, "\u{f42f}", ""),     // oct-mail
    (ALERT, "\u{f421}", "!"),      // oct-alert
    (ONLINE, "\u{f0318}", ""),     // md-lan_connect
    (OFFLINE, "\u{f0319}", "✕"),   // md-lan_disconnect
];

/// Makes muttrs' icons known to `kit`.
pub(crate) fn define(kit: &mut Kit) {
    for (name, nerd, plain) in ICONS {
        kit.define_icon(name, nerd, plain);
    }
}

/// The icon of a Mailbox with `role`.
pub(crate) fn role(role: Option<muttrs_mail::Role>) -> &'static str {
    use muttrs_mail::Role;
    match role {
        Some(Role::Inbox) => INBOX,
        Some(Role::Sent) => SENT,
        Some(Role::Drafts) => DRAFTS,
        Some(Role::Trash) => TRASH,
        Some(Role::Junk) => JUNK,
        Some(Role::Archive) => ARCHIVE,
        Some(Role::All) => ALL,
        Some(Role::Flagged) => FLAGGED,
        None => FOLDER,
    }
}
