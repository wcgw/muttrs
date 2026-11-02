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

### 2. Rendering (muttrs-render)

- [ ] HTML: whitespace, blocks and inlines, headings, paragraphs, lists, block quotes, `pre`, `hr`, `br`
- [ ] HTML: inline styles and `<style>` rules (color, weight, style, decoration, `display:none`, alignment), colours
      adapted to the theme's background
- [ ] HTML: what newsletters hide (preheaders, `mso-hide`, zero-size text, spacer characters)
- [ ] HTML: tables, as grids when they hold data, linearised when they lay out a page
- [ ] HTML: Links numbered, as w3m does; images as slots sized from attributes or their pixels; tracking pixels dropped
- [ ] Plain text: quotes by depth, signatures, `format=flowed`, URLs as Links

### 3. Pictures and the Pager on a file

- [ ] muttrs-gfx: kitty Unicode placeholders and half blocks, chosen for the terminal
- [ ] `muttrs view FILE.eml`: headers, body, inline (`cid:`) images, Remote Content on request, Links, Attachments,
      HTML/plain toggle, search
- [ ] Theme, Nerd Font icons with plain fallbacks, configuration file

### 4. Accounts over IMAP (muttrs-mail)

- [ ] Configuration of Accounts, with presets for Gmail and Outlook
- [ ] TLS: implicit (`ssl`) and `starttls`, platform roots plus an optional CA file
- [ ] Authentication: password from the keyring or a command; OAuth2 (XOAUTH2), with `muttrs login` doing the browser
      flow (PKCE, loopback redirect) and keeping the refresh token in the keyring
- [ ] Mailboxes: LIST with SPECIAL-USE Roles, modified UTF-7 names, STATUS counts
- [ ] Envelopes of the newest Messages; bodies fetched on open, cached on disk
- [ ] Flags (read, flagged), move, delete to Trash, archive; MOVE or COPY + EXPUNGE
- [ ] New mail by IDLE, reconnecting with backoff

### 5. The TUI over Accounts

- [ ] Sidebar of Accounts and Mailboxes, Index, Pager, status line, `:` commands, help
- [ ] Keys after mutt and vim (see the README)

### 6. Writing (muttrs-mail + muttrs)

- [ ] Compose, reply, reply all, forward, in `$EDITOR`, headers on top of the body
- [ ] Markdown bodies sent as plain text plus HTML
- [ ] Attachments
- [ ] SMTP over `ssl` or `starttls`, password or XOAUTH2; a copy in Sent unless the server keeps one (Gmail)
- [ ] Postponing to Drafts, and resuming

### Later

- Threads (client-side, after JWZ), search on the server (`X-GM-RAW` on Gmail), an Envelope cache with CONDSTORE
- Several Accounts side by side in one sidebar (the configuration already takes several)
- Pictures inline in text (icons), animated GIFs, syntax highlighting in `<pre>`
- PGP/S-MIME, notmuch

## Testing

- muttrs-render: unit tests on HTML and text fixtures, asserting the text and styles drawn.
- muttrs-gfx: the escape sequences, the placeholder cells, the half blocks' colours.
- muttrs-mail: MIME and modified UTF-7 unit tests; OAuth2 against a mock token endpoint; IMAP and SMTP against a local
  test server in a container ([docs/development.md](development.md)), over both TLS modes.
- muttrs: state and keys, and drawing to ratatui's test backend.
