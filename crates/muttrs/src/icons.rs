//! The marks muttrs draws: Nerd Font icons (Octicons and Material Design, as named in Nerd Fonts' `glyphnames.json`),
//! or plain characters where there's no Nerd Font.
//!
//! Icons are Private Use Area characters that a Nerd Font may draw wider than a cell, so each is followed by a space
//! to spill into. Codepoints were checked against the glyph names of a Nerd Fonts 3 font (CaskaydiaCove).

/// The marks to draw with.
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Until the Index and the sidebar draw them.
pub(crate) struct Icons {
    /// A Message that's Unread.
    pub(crate) unread: &'static str,
    /// A Message that's Flagged.
    pub(crate) flagged: &'static str,
    /// A Message that's been answered.
    pub(crate) answered: &'static str,
    /// A Message with Attachments.
    pub(crate) attachment: &'static str,
    /// An image not drawn, before its description.
    pub(crate) image: &'static str,
    /// A Link.
    pub(crate) link: &'static str,
    /// Remote Content, blocked.
    pub(crate) blocked: &'static str,
    /// The sender, in the Pager's headers.
    pub(crate) from: &'static str,
    /// The recipients.
    pub(crate) to: &'static str,
    /// The date.
    pub(crate) date: &'static str,
    /// Mailboxes by Role.
    pub(crate) inbox: &'static str,
    pub(crate) sent: &'static str,
    pub(crate) drafts: &'static str,
    pub(crate) trash: &'static str,
    pub(crate) junk: &'static str,
    pub(crate) archive: &'static str,
    pub(crate) all: &'static str,
    pub(crate) folder: &'static str,
    /// An Account, in the sidebar.
    pub(crate) account: &'static str,
    /// Waiting on the server.
    pub(crate) loading: &'static str,
    /// Something went wrong.
    pub(crate) alert: &'static str,
    /// Connected and up to date.
    pub(crate) online: &'static str,
    /// Not connected.
    pub(crate) offline: &'static str,
}

impl Icons {
    pub(crate) const NERD: Self = Self {
        unread: "\u{f444} ",     // oct-dot_fill
        flagged: "\u{f51f} ",    // oct-star_fill
        answered: "\u{f4a8} ",   // oct-reply
        attachment: "\u{f4fb} ", // oct-paperclip
        image: "\u{f4e5} ",      // oct-image
        link: "\u{f44c} ",       // oct-link
        blocked: "\u{f511} ",    // oct-shield_lock
        from: "\u{f415} ",       // oct-person
        to: "\u{f4fd} ",         // oct-people
        date: "\u{f43a} ",       // oct-clock
        inbox: "\u{f48d} ",      // oct-inbox
        sent: "\u{f4fa} ",       // oct-paper_airplane
        drafts: "\u{f448} ",     // oct-pencil
        trash: "\u{f48e} ",      // oct-trash
        junk: "\u{f421} ",       // oct-alert
        archive: "\u{f411} ",    // oct-archive
        all: "\u{f51e} ",        // oct-stack
        folder: "\u{f413} ",     // oct-file_directory
        account: "\u{f42f} ",    // oct-mail
        loading: "\u{f46a} ",    // oct-sync
        alert: "\u{f421} ",      // oct-alert
        online: "\u{f0318} ",    // md-lan_connect
        offline: "\u{f0319} ",   // md-lan_disconnect
    };

    pub(crate) const PLAIN: Self = Self {
        unread: "● ",
        flagged: "★ ",
        answered: "↩ ",
        attachment: "+ ",
        image: "image: ",
        link: "→ ",
        blocked: "⊘ ",
        from: "",
        to: "",
        date: "",
        inbox: "",
        sent: "",
        drafts: "",
        trash: "",
        junk: "",
        archive: "",
        all: "",
        folder: "",
        account: "",
        loading: "… ",
        alert: "! ",
        online: "",
        offline: "✕ ",
    };
}
