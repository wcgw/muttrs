# muttrs

A terminal mail client after mutt and vim, that renders HTML Messages, images included, as well as a terminal can.

## Language

### Where mail lives

**Account**:
One mail identity the user reads and sends as: an address, the IMAP server its mail is read from, the SMTP server it's
sent through, and how to sign in to both.
_Avoid_: profile, login, identity

**Mailbox**:
A named collection of Messages on an Account's IMAP server. Gmail's labels are Mailboxes.
_Avoid_: folder, label

**Role**:
What a Mailbox is for, when it has a special use: Inbox, Sent, Drafts, Trash, Junk, Archive, All. The server says so
(SPECIAL-USE), or the configuration does.
_Avoid_: special folder, type

**Message**:
One email in a Mailbox, identified there by its UID (valid as long as the Mailbox's UIDVALIDITY is).
_Avoid_: mail, email, item

**Envelope**:
What the Index shows of a Message without fetching it: date, sender, recipients, subject, Flags, size, and whether it
has Attachments.
_Avoid_: header, summary

**Flags**:
The marks a Message carries: Seen, Flagged, Answered, Draft, Deleted.

**Unread**:
A Message without the Seen Flag.
_Avoid_: new (that's mutt's word, and IMAP's Recent)

**Flagged**:
A Message the user marked as important; Gmail calls it starred.
_Avoid_: starred, important

### Reading

**Index**:
The list of the Messages of the open Mailbox, newest first.
_Avoid_: list, inbox (the Inbox is a Mailbox)

**Pager**:
Where one Message is read: its headers, then its body as rendered, with its Pictures, Links and Attachments.
_Avoid_: preview, viewer, reader

**Rendering**:
A Message's body laid out for the Pager's width: styled lines, the Links in them, and slots for its images.

**Part**:
One piece of a Message's MIME structure.

**Attachment**:
A Part meant to be saved or opened apart from the body, rather than shown in it.
_Avoid_: file

**Inline Image**:
A Part an HTML body shows, by `cid:` reference. It's no Attachment.

**Remote Content**:
What an HTML body would load from the network, images mostly. Loading it tells the sender the Message was read, so
it's blocked until the user asks for it.
_Avoid_: external images

**Tracking Pixel**:
An image no one is meant to see (one pixel, or hidden) whose only use is to tell the sender the Message was opened.
Never loaded, never drawn.

**Link**:
A URL in a body, numbered where it appears, which the user opens by its number.

**Picture**:
An image drawn in the terminal's cells: through kitty's Unicode placeholders, or as half blocks.
_Avoid_: image (that's what's in the Message), graphic

### Writing

**Draft**:
A Message the user is writing, edited in `$EDITOR` as headers then a body in Markdown.
_Avoid_: composition

**Postponed**:
A Draft saved to the Drafts Mailbox, to finish later.

## Relationships

- An **Account** has many **Mailboxes**; at most one **Mailbox** of an **Account** has each **Role**.
- A **Message** belongs to one **Mailbox**. The same email in two Gmail labels is two **Messages**, one in each.
- The **Index** shows the **Envelopes** of one **Mailbox**; the **Pager** shows one **Message** of it.
- A **Message**'s body is one **Part**, or chosen among alternatives (HTML or plain text); its **Inline Images** and
  **Attachments** are other **Parts**.
- A **Rendering** belongs to a **Message**, a width and a theme: it's made again when any changes, or when an image's
  size becomes known.
- **Remote Content** is loaded per **Message**, never for a **Tracking Pixel**.

## Flagged ambiguities

- "new" means **Unread** to mutt users and Recent to IMAP. muttrs says **Unread** and ignores Recent.
- Deleting a **Message** moves it to the **Mailbox** with the Trash **Role**; only deleting from Trash expunges it.
  On Gmail, flagging a **Message** Deleted without moving it only removes it from that label.
- Archiving moves a **Message** to the Archive **Mailbox**, or on Gmail out of the Inbox (to All Mail), where it stays
  in its other labels.
