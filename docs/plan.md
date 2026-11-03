# Plan

muttrs is a terminal mail client: mutt's index, pager and compose flow, driven with vim's keys, that renders HTML
Messages, images included, as well as a terminal can. It reads mail over IMAP and sends it over SMTP, each over
implicit TLS or STARTTLS, and signs in to Gmail with OAuth2. It is built and tested on Linux and must stay portable to
macOS. The words used here are defined in [CONTEXT.md](../CONTEXT.md).

This file is the roadmap: what's built, what's next, and why it's shaped the way it is. Decisions with lasting
consequences get a record of their own in [`adr/`](adr).

## Architecture

A Cargo workspace, layered so that each crate only knows the ones below it:

```
muttrs            the binary: configuration, theme, icons, the TUI (sidebar, index, pager, compose), its keys
├── muttrs-mail   Accounts and their protocols: IMAP (reading), SMTP (sending), TLS, OAuth2, secrets, MIME
├── muttrs-render a Message's body, HTML or plain text, laid out as styled lines for a width, with its Links and
│                 the slots its images go in
└── muttrs-gfx    Pictures in the terminal: kitty graphics through Unicode placeholders, or half blocks
```

- **muttrs-render** is pure: HTML or text in, lines out, given a width, a style sheet and what's known of the images'
  sizes. No terminal, no network. It's the part with the most tests. See
  [ADR 0001](adr/0001-own-html-layout.md).
- **muttrs-gfx** draws Pictures as ordinary ratatui cells, so they scroll and clip with the text around them: kitty
  Unicode placeholders where the terminal has them (Ghostty, kitty), half blocks in truecolor everywhere else. See
  [ADR 0002](adr/0002-pictures-as-cells.md).
- **muttrs-mail** owns every connection. The TUI talks to an Account through a worker task that holds the IMAP session
  and answers requests over channels, so the UI never waits on the network. A second session IDLEs for new mail. See
  [ADR 0003](adr/0003-imap-smtp-and-gmail.md).
- **muttrs** keeps the state, draws it, and turns keys into requests. `muttrs view FILE.eml` opens the Pager on a file
  without any Account, which is how rendering is tried out.

Portability to macOS: TLS is rustls with the ring provider and the platform's roots (no OpenSSL); secrets go through
`secret-tool` on Linux and `security` on macOS, or any command; files follow XDG on both; URLs and attachments open
with `xdg-open` or `open`; kitty images are sent in-band where `/dev/shm` doesn't exist. CI builds on both.

## Milestones

Each lands as one or more commits, with its docs.

### 1. Skeleton ✅

- [x] Workspace, lints, CI for Linux and macOS
- [x] README, CONTEXT, this plan, ADRs for the three decisions above

### 2. Rendering (muttrs-render) ✅

- [x] HTML: whitespace, blocks and inlines, headings, paragraphs, lists, block quotes, `pre`, `hr`, `br`
- [x] HTML: inline styles and `<style>` rules (color, weight, style, decoration, `display:none`, alignment), colours
      adapted to the theme's background
- [x] HTML: what newsletters hide (preheaders, `mso-hide`, zero-size text, spacer characters)
- [x] HTML: tables, as grids when they hold data, linearised when they lay out a page
- [x] HTML: Links numbered, as w3m does; images as slots sized from attributes or their pixels; tracking pixels dropped
- [x] Plain text: quotes by depth, signatures, `format=flowed`, URLs as Links

Two rules came from trying it on newsletters: centred text that wraps is a paragraph, and goes left (headings,
buttons and footers stay centred); and list items are never centred, which would part them from their markers.
`cargo run -p muttrs-render --example render -- FILE.html [WIDTH]` prints a file's Rendering.

### 3. Pictures and the Pager on a file ✅

- [x] muttrs-gfx: kitty Unicode placeholders and half blocks, chosen for the terminal
- [x] `muttrs view FILE.eml`: headers, body, inline (`cid:`) images, Remote Content on request, Links, Attachments,
      HTML/plain toggle, search
- [x] Theme, Nerd Font icons with plain fallbacks, configuration file

HTML sizes images in CSS pixels, and HiDPI terminals report cells in device pixels (about 20 × 40 on a 2× screen),
which drew images at half their size. The pixel ratio is guessed from the cells' height, and can be configured.
Tried in tmux (half blocks); kitty placeholders are tested by their escape sequences, not yet on screen.

### 4. Accounts over IMAP (muttrs-mail) ✅

- [x] Configuration of Accounts, with presets for Gmail and Outlook
- [x] TLS: implicit (`ssl`) and `starttls`, platform roots plus an optional CA file; plain text to the loopback only
- [x] Authentication: password from the keyring or a command; OAuth2 (XOAUTH2, sent as SASL-IR first), with
      `muttrs login` doing the browser flow (PKCE, loopback redirect) and keeping the refresh token in the keyring
- [x] Signing in again from the UI (`:login`), and from a browser on another machine, by pasting the address it ends
      at
- [x] Mailboxes: LIST with SPECIAL-USE Roles, modified UTF-7 names, STATUS counts
- [x] Envelopes of the newest Messages (300), from `HEADER.FIELDS`; bodies fetched on open, cached on disk
- [x] Flags (read, flagged), move, delete to Trash, archive; MOVE or COPY + EXPUNGE
- [x] New mail by IDLE (renewed every 25 minutes), reconnecting with backoff; a refused command fails alone

Tested against GreenMail and Dovecot in containers ([development.md](development.md#mail-servers-to-test-against)):
TLS, STARTTLS, XOAUTH2, refusals, and an Account's client end to end, IDLE included. Not tested against Gmail or
Outlook themselves.

### 5. The TUI over Accounts ✅

- [x] Sidebar of Accounts and Mailboxes (Roles first, then the hierarchy), Index, Pager, status line, `:` commands,
      help
- [x] Keys after mutt and vim (see the README); flags, deleting and archiving show at once, before the server says
- [x] `muttrs login NAME`

Tried in tmux against the test servers: reading, stepping with `J`/`K`, flagging, deleting to Trash, `:cd`.

### 6. Writing (muttrs-mail + muttrs) ✅

- [x] Compose, reply, reply all, forward, in `$EDITOR` (a `.eml` file, so vim edits it as mail), headers on top of
      the body; the TUI lets go of the terminal while the editor has it
- [x] Markdown bodies sent as plain text plus HTML, new lines breaking lines; raw HTML in a Draft stays text
- [x] Attachments: `Attach:` lines, `a` and `D` on the Draft; a forward keeps the original's
- [x] SMTP over `ssl` or `starttls`, password or XOAUTH2; a copy in Sent unless the server keeps one (Gmail); what's
      answered flagged so
- [x] Postponing to Drafts

Tried in tmux with Neovim as the editor, sending through the test servers: replies arrive with `In-Reply-To`, both
bodies and their Attachments, and land in Sent. Postponing needs a recipient, as building the Message does.

Servers with certificates of their own (mail bridges): `server_certificate` trusts the certificates named, exactly,
and `muttrs trust NAME` shows and keeps what the servers present. Tested in-process with a self-signed CA certificate
(webpki's `CaUsedAsEndEntity`, as Proton Mail Bridge's gives), and against the test servers over STARTTLS and TLS.

### 7. Next

- [x] Resuming a Postponed Draft (`e` in Drafts): its headers, Bcc too, Markdown and Attachments, the copy kept
      expunged once it's sent or postponed again
- [x] Threads in the Index (client-side, after JWZ, without grouping by subject): replies under what they reply
      to, threads by their latest Message, folding (`za`, `zM`, `zR`); `T` switches; `hide_thread_subjects` draws
      a reply's subject as mutt's arrow where it's what it replies to's
- [x] Gmail's own threads (`X-GM-THRID`), for Messages whose References don't say: threads Gmail has as one
      are joined under the earliest
- [x] Search on the server (`S`): `SEARCH TEXT`, and Gmail's own syntax through `X-GM-RAW`; the newest 300 found
      listed as an Index of their own until `Esc`
- [x] More of a Mailbox than its newest 300 Envelopes, 300 at a time as the Index's cursor nears the end
- [x] An Envelope cache: the Mailboxes and each Mailbox's Envelopes kept, shown at once (before the server says) and
      brought up to date rather than fetched again; offline, what's cached can be listed and read
- [ ] CONDSTORE/QRESYNC, to bring the cache up to date by what changed rather than by every listed Message's Flags
- [x] Moving to a Mailbox by name (`s`, narrowed as it's typed), and Selections (`Space`) to act on several
      Messages; a folded thread is acted on whole
- [x] Address completion from the Envelopes and Messages seen: `m` asks To (`Tab` completes) and Subject before the
      editor; the addresses are kept in a file the editor can complete from too
- [x] Signatures (`signature`, `signature_file`), above the quote; `identities`, replies from the one written to, and
      left out of reply-all; a new Draft left as it was is dropped
- [x] The mouse: the wheel scrolls what it's over, a click chooses, a second opens; themes from files, over a
      built-in theme (Catppuccin Mocha shipped as the example)
- [x] `:select` and `:unselect` with a Filter (`from:`, `to:`, `cc:`, `subject:`, `is:`, `has:attachment`, `-`), as
      ghd's; `Tab` completes the command, the qualifier and its values from what's listed
- [x] The Index above the Pager, as mutt's `pager_index_lines` (a click there opens the Message clicked), and
      NeoMutt's `pager_read_delay`: a Message is marked read once shown that long (1 second by default)

### Later

- Pictures inline in text (icons), animated GIFs, syntax highlighting in `<pre>`
- PGP/S-MIME, notmuch
- iTerm2's inline images, on macOS

## Testing

- muttrs-render: unit tests on HTML and text fixtures, asserting the text and styles drawn.
- muttrs-gfx: the escape sequences, the placeholder cells, the half blocks' colours.
- muttrs-mail: MIME and modified UTF-7 unit tests; OAuth2 against a mock token endpoint; IMAP and SMTP against a local
  test server in a container ([docs/development.md](development.md)), over both TLS modes.
- muttrs: state and keys, and drawing to ratatui's test backend.
