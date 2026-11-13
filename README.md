# muttrs

A terminal mail client after mutt and vim, that renders HTML mail, images included, as well as a terminal can.

muttrs reads mail over IMAP and sends it over SMTP, each over TLS or STARTTLS, and signs in to Gmail with OAuth2. The
words used here, and in the code, are defined in [CONTEXT.md](CONTEXT.md); what's built and what's next is in
[docs/plan.md](docs/plan.md).

> Work in progress: reading, threads and writing work; searching the server is next. See the plan.

## Requirements

- Linux or macOS, and Rust 1.88 or newer to build it.
- To keep passwords and OAuth2 tokens in the keyring: `secret-tool` (from libsecret) and a Secret Service (GNOME
  Keyring, KeePassXC, …) on Linux; `security`, which macOS has, on macOS. Or `password_command`, for any other.
- For Pictures at full resolution, a terminal with kitty's Unicode placeholders: [Ghostty](https://ghostty.org) or
  [kitty](https://sw.kovidgoyal.net/kitty/). Any other terminal draws them with half blocks.
- For icons, a [Nerd Font](https://www.nerdfonts.com). Ghostty, kitty and WezTerm have the symbols built in.

## Install

```sh
cargo install --locked --path crates/muttrs
```

muttrs runs in [wcgw-kit](https://github.com/wcgw/wcgw-kit), the editor shell the wcgw apps share: until the kit is
published, it's built from a checkout of it beside muttrs' (`../wcgw-kit`), and voight from the `wcgw-kit` branch of
a local checkout of voight.

That builds [voight](https://github.com/wcgw/voight) in, as ghd does, running on the CPU: it estimates how much of
the prose the Pager shows an LLM wrote (below). Add `--features cuda` to run it on an NVIDIA GPU (this needs the CUDA
toolkit's `nvcc`), `--features metal` on a Mac's GPU (on Apple silicon, voight builds with Metal anyway, unless with
wgpu), or `--features wgpu` on any other GPU, through Vulkan on Linux; wgpu is experimental, on a fork of candle. Or
leave voight out with `--no-default-features`.

The first time it starts with voight, muttrs downloads voight's model, about 1.7 GB, into the Hugging Face cache
(`~/.cache/huggingface`, or `$HF_HOME`), and loads it each time it starts, on a thread of its own. Set
`voight = "off"` to start without it.

## Accounts

Each Account is an `[[account]]` in the configuration (below). For your own server:

```toml
[[account]]
name = "home"                                         # in the sidebar, and in `muttrs login home`
email = "alex@example.com"
display_name = "Alex"
imap = { host = "imap.example.com", tls = "ssl" }     # "ssl" (993) or "starttls" (143); port = … for others
smtp = { host = "smtp.example.com", tls = "starttls" } # "ssl" (465) or "starttls" (587)
# username = "alex"                                   # the address, by default
# password_command = "pass show mail/home"            # without it, the password is in the keyring
# mailboxes = { archive = "Archive" }                 # Roles the server doesn't say (SPECIAL-USE)
# identities = ["Alex <alex@work.example>"]           # other addresses it sends as: replies are from the one written to
# signature = "Alex\nexample.com"                      # or signature_file = "~/.signature"
```

then `muttrs login home` asks for the password and keeps it in the keyring (the Secret Service through
`secret-tool` on Linux, the login Keychain on macOS), and checks it signs in.

### Servers with a certificate of their own

A server whose certificate no CA signed, as a mail bridge's (Proton Mail Bridge's, say) or one you run, is refused:
its certificate isn't trusted. To trust it as it is,

```sh
muttrs trust home
```

shows the SHA-256 fingerprints of the certificates the Account's servers present, keeps them once you say so (in
`~/.config/muttrs/certificates/home.pem`), and says the line to add to the Account:

```toml
server_certificate = "~/.config/muttrs/certificates/home.pem"
```

Compare the fingerprints with what the server says its certificate is first: for Proton Mail Bridge, export its
certificate from its settings, and `openssl x509 -noout -fingerprint -sha256 -in cert.pem`; or point
`server_certificate` at that file directly. The servers may then present those certificates and no other (the
handshake still checks they hold their keys), whoever signed them, whatever names they're for and however old they
are. For servers whose certificates a CA of your own signs, `ca_file = "ca.pem"` trusts that CA instead.

### Gmail

```toml
[[account]]
name = "gmail"
email = "alex@gmail.com"
provider = "gmail"           # Gmail's servers, OAuth2, and its quirks

[account.oauth2]
client_id = "….apps.googleusercontent.com"
client_secret = "…"          # or client_secret_command = "…"
```

Gmail signs in with OAuth2, and muttrs ships no OAuth client of its own: each user brings theirs. In the
[Google Cloud console](https://console.cloud.google.com/apis/credentials), create a project, configure its OAuth
consent screen (an external app, with yourself as a test user), and create an OAuth client ID of type "Desktop app".
Its id and secret go above. Then `muttrs login gmail` opens the browser to sign in, and keeps the refresh token in
the keyring. Google lets the refresh tokens of apps still "in testing" last 7 days, unless you publish the app; when
one runs out, `:login gmail` signs in again without leaving muttrs. With an
[app password](https://myaccount.google.com/apppasswords) instead, set `auth = "password"`.

Once you've signed in, the browser comes back to muttrs, at an address on `127.0.0.1`. A browser on another machine
(muttrs over SSH, say) can't: it shows an error page instead. Copy that page's address, from the address bar, and
paste it into muttrs, which takes the code from it. `muttrs login` prints the address to sign in at, and `:login`
shows it, for `yy` to yank, and asks for the address pasted.

Gmail's labels are Mailboxes; deleting moves to its Trash, archiving (`a`) takes a Message out of the Inbox, and
what's sent isn't copied to Sent, as Gmail does that itself. `provider = "outlook"` does the same for Outlook and
Microsoft 365, with a client registered in Microsoft Entra (untested so far).

## Keys

muttrs runs in [wcgw-kit](https://github.com/wcgw/wcgw-kit), as devnull, ghd and slackers do: its keys are vim's,
pure (`j` `k` `gg` `G` `<C-d>` `<C-u>` `/` `?` `n` `N` `]]` `[[` `zc` `zo` `za` `zM` `zR` `v` `yy` `<C-o>` `<C-w>h`…),
and every action is a `:` command, which `:map` can put on any key. `<Space>` is `<leader>`: what may follow it shows
after a moment. `:help` (or `F1`) lists the keys and the commands.

| Where | Keys |
|---|---|
| Anywhere | `:q` closes a window · `:qa`, or `q` where there's nothing to close (the Pager, the Draft, what a search found), quits once what's asked is done; `:qa!` quits now · `<C-l>` `:e!` ask for what's new · `:sync` list the open Mailbox anew · `!` `:shell CMD` run a command · `\` `:sidebar` go to the sidebar, hide it from within it, or bring it back · `<Space>ff` find a Mailbox, `:e NAME` open one by name · `:create NAME` `:rename NAME` `:delmailbox NAME` make, rename (the open one), or delete a Mailbox · `:login NAME` sign in again, in the browser |
| Sidebar | `<CR>` open (on an Account, ask what's new) · `zc` `zo` fold an Account, or a Mailbox holding others · `<C-w>l` the Index |
| Index | `<CR>` (or a double click) read · `/` `?` `n` `N` search what's listed · `S` `:grep` search the server · `od` `of` `os` `oz` `ot` sort by date, sender, subject, size, or in threads; `O` the other way · `za` `zM` `zR` fold the thread, all, none · `x` select, and down · `A` all, or none · `<Esc>` hide the search's matches, or unselect all, or else list all · `s` flag · `U` Unread or read · `<Space>l` Labels · `dd` delete (to Trash) · `a` archive · `<Space>m` move to a Mailbox · `u` undo that · `<Space>j` `<Space>J` `:junk` `:notjunk` to Junk, or out of it · `\|` pipe to a command · `K` the Envelope · `<C-w>h` the sidebar |
| Pager | the motions scroll · `<C-n>` `<C-p>` the next Message, the previous · `<Space>b` `:body` HTML or plain text · `K` every header, as written · `:raw` the Message as it came · `I` `:remote` load Remote Content · `<Space>h` voight's Heatmap · `<CR>` on a Link, `gx` open it · `gy` yank it · `<Space>o` Links (`<C-y>` yanks one) · `<Space>a` Attachments (`<C-s>` saves one) · `s` `U` `<Space>l` `dd` `a` `<Space>m` `u` `\|` as in the Index · `:unsubscribe` from its mailing list · `q` back |
| Writing, in the Index or the Pager | `c` a new Message · `r` reply · `R` reply to all · `<Space>L` reply to the mailing list · `<Space>F` forward · `:forward attached` forward as an Attachment · `:bounce TO` send it on as it is · `:resume` resume a Postponed Draft, in Drafts |
| A Draft | `<CR>` `:draft` edit again · `<Space>s` `:send` send · `:attach FILE` attach a file · `:detach` detach the last · `<Space>p` `:postpone` postpone to Drafts · `q` discard, once you say so |

### Keys of your own

`~/.config/muttrs/init.vim` (after `~/.config/wcgw/init.vim`, which every wcgw app reads) says them, as vim's does:

```vim
nnoremap X :archive<CR>          " X archives too
nnoremap <C-a> :e Archive<CR>    " Control-a opens the Archive
let mapleader = ","              " or set the leader once, for all of them, in ~/.config/wcgw/init.vim
```

`[keys.*]` in config.toml are no longer read: `scripts/convert-config.py config.toml` writes them as `:nnoremap` lines.

The sidebar lists each Account and its Mailboxes. With several Accounts, their Inboxes come first, under `Inboxes`,
each by its Account's name, in the Accounts' order.

`s`, `U`, `<Space>l`, `dd`, `a` and `<Space>m` act on the Messages selected with `x`, if any, or else on the one at the
cursor; on a folded thread, on all of it. `<Space>m` lists the Account's Mailboxes, narrowed as you type their name.
`u` moves the Messages last deleted, archived or moved back where they were, and the ones before them as it's pressed
again, found where they went by their `Message-ID`; on Gmail, a Message archived goes back to the Inbox as a copy, as
taking it out of All Mail would delete it. A server can take a while to show them where they went (Proton's Bridge,
archiving): muttrs looks again for half a minute, and if they're still not there, `u` tries again later. What's
deleted from Trash is deleted for good: there's no undoing that.

What muttrs asks of the server, it asks in the background, one Task at a time, in the order asked (a Message to read
first): what you flag, delete, archive, move or label shows done at once, and stays so while the server catches up.
Once a Task takes a while, the statusline says it, with how many more wait: `⠹ Archiving 3 Messages  2 more`.
`:send` sends a Draft in the background too: it goes at once, and comes back, with why, if it couldn't be sent. `q`
and `:qa` with Tasks left quit once they're done, unless you press a key first; `:qa!` quits now
([docs/adr/0006-tasks.md](docs/adr/0006-tasks.md)).

`:select` and `:unselect` select, or unselect, the Messages listed that a filter picks, folded or not (all of them,
without one). A filter is written as Gmail's search is: `from:`, `to:` and `cc:` (part of a name or an address),
`subject:` (part of it, in double quotes if it has spaces), `label:` (a Label's whole name), `is:unread`, `is:read`,
`is:flagged`, `is:answered`, `is:draft` and `has:attachment`; `-` before a term leaves out what it picks. A qualifier
repeated picks either, as in `from:ann from:bob`; different ones, both, as in `from:ann is:unread`. `Tab` completes
the filter's qualifier, then its value: the addresses of the Messages listed, their Labels, or `is:`'s values that
would pick some of them. `:sort date` (or `from`, `subject`, `size`, `threads`) sorts the Index as mutt's `o` does,
`:sort!` the other way; sorted otherwise than by date or in threads, it's out of threads. `:threads` puts it in
threads, or newest first.

`:junk` moves the Messages an action is for to Junk, marked `$Junk`, as spam; in Junk, `:notjunk` moves them to the
Inbox, marked `$NotJunk`. Spam filters learn by one or the other: the move (Dovecot's, by IMAPSieve), or the keyword.
`u` undoes either, taking the keyword off. `:bounce TO` sends them on as they are, under `Resent-` headers, without a
copy in Sent, as mutt's `b` does; `:forward attached` forwards one as an Attachment of its own (`message/rfc822`)
rather than quoted. `:create Work/Reports` makes a Mailbox, `:rename NAME` renames the open one, and `:delmailbox
NAME` deletes one, its Messages with it; those with a Role (the Inbox, Trash and the like) keep their names, and
stay.

`|` pipes Messages to a command: the one the Pager shows, as it came, or those selected (else the one at the cursor,
a folded thread whole), several as an mbox, the oldest first, as `git am` and `b4 am` take a series of patches:
`|git am -3`. The command has the terminal until `Enter` brings muttrs back. `!` runs a command so, or the shell. The
Messages are piped byte for byte, an 8bit part in Latin-1 as it came.

`:filter` lists only the Messages a filter picks, in threads among themselves, until `:filter` alone lists them all
again; `:select` then picks among those. `<Esc>` (`:clear`) unselects every Message, listed or not, or with none
selected, lists them all again.

After `:` and `/`, `Up` and `Down` go back through what was typed before that starts with what's typed, as in vim.
wcgw-kit keeps it in `~/.local/state/muttrs/history` (or under `$XDG_STATE_HOME`), the newest 200 of each, where it's
always been, so what was typed before carries over; what `!` and `|` ran is no longer kept.

## Labels

A Label is a Mailbox a Message can be in as well as the one it's listed in: on Gmail, its own labels; elsewhere, the
Mailboxes under `Labels`, as Proton Mail's Bridge keeps them. The Index shows a Message's Labels before its subject,
and `:select label:NAME` picks by them. `<Space>l` lists the Account's Labels, marked `✓` where the Messages it acts
on all have one and `–` where some do; `<CR>` adds a Label to all of them, or takes it off all of them if they all
have it, the list staying open for more, until `<Esc>`. Typing narrows the list, as `<Space>m`'s.

On Gmail, the Labels come with the Messages (`X-GM-LABELS`). Elsewhere, a Message is labelled by copying it to the
Label's Mailbox, and its Label taken off by expunging that copy; muttrs finds which Messages each Label's Mailbox
has by their `Message-ID`, asking each time a Mailbox is opened, and listing again only those that changed since; a
Message without a `Message-ID` shows none. On the Bridge, taking a Label off this way leaves the Message in its other
Mailboxes, as moving it out of the Label's would ([docs/adr/0005-labels.md](docs/adr/0005-labels.md)).

## voight

With voight on, the statusline says how much of the prose the Pager shows voight estimates an LLM wrote (`voight
73%`), once it has assessed it, one Message at a time. What's quoted is left out, as it's someone else's, and so is
preformatted text; a body with fewer than 50 words of prose isn't assessed. `<Space>h` (`:voight heatmap`) shows the
Heatmap, or hides it: each stretch of prose voight scored (about 100 to 400 words) shaded from green (0%) through
yellow to red (100%), in the colours of the `VoightHeatHuman`, `VoightHeatEven` and `VoightHeatLlm` highlight groups.
It's an estimate, never evidence of authorship. `:voight off` stops assessing, and `:voight on` starts again, as in
ghd.

## Searching

`/` searches what the Index lists, as you'd search in vim. `S` (`:grep QUERY`) searches the whole Mailbox on the
server, for text anywhere in its Messages, and lists the newest 300 it finds as an Index of their own until `q`. On
Gmail, it takes Gmail's own syntax: `from:ann has:attachment after:2026/10/01`. `:search QUERY` searches every
Mailbox of the Account, and lists those it found Messages in, how many in each; choosing one lists what it found
there, as `S` does.

## Writing

`c` asks whom the Message is to (`Tab` completes addresses, from those muttrs has seen), then its subject, as mutt
does; `:compose ADDRESS` says whom to at once, and completes the same. Then `c`, `r`, `R` and `<Space>F` open a Draft
in your editor (`editor` in the configuration, else `$EDITOR`, `$VISUAL`, `nvim` or `vi`) as a file of headers, a
blank line, then the body:

```
From: Alex <alex@example.com>
To: Ann <ann@example.com>
Cc:
Bcc:
Subject: Re: Lunch
Attach: ~/menu.pdf

Thursday works, see you at **noon**.

On Mon 2 Nov 2026 at 09:30, Ann <ann@example.com> wrote:
> Lunch on Thursday?
```

Addresses complete from those muttrs has seen; from a command, as mutt's `query_command` (`query_command = "khard
email --parsable %s"`, or `"abook --mutt-query %s"`: `%s` is what's typed, and it prints a line of its own, then one
an address, `address<TAB>name`), whose addresses come first once it answers; and from an Account's address book on a
CardDAV server, read when muttrs starts:

```toml
[account.carddav]
url = "https://dav.example.com/addressbooks/alex/contacts/" # the address book's collection
# username = "alex"                                         # the Account's, by default
# password_command = "pass show dav"                        # the Account's password, by default
```

The addresses muttrs has seen are kept in `~/.cache/muttrs/addresses.txt`, one a line, which vim and Neovim can
complete from in the Draft's headers too (`Ctrl-X Ctrl-K`), with `autocmd FileType mail setlocal
dictionary+=~/.cache/muttrs/addresses.txt iskeyword+=@-@,.,-`.

`<Space>L` (`:reply list`) replies to the mailing list a Message came through, to the list alone (its `List-Post`), as mutt's list-reply
does. `:unsubscribe`, in the Pager, unsubscribes from that list as the Message says to (`List-Unsubscribe`): by a
POST alone where it says one does (RFC 8058), else by mail, sent at once, else at the list's page, in the browser.

The Account's signature goes in after a `-- ` line, above the quote in a reply. A reply is from the address of the
Account's (its own, or one of its `identities`) the Message was sent to. Quitting the editor on a new Draft without
changing it drops the Draft, as mutt does. Its name ends in `.eml`, so that vim and Neovim edit it as mail. The body is
Markdown, sent as it is (the plain text) and as the HTML it
makes, in which each new line breaks the line, as in plain text mail. `Attach:` lines attach files. Saving and
quitting shows the Draft as it will be sent, until `<Space>s` (`:send`) sends it through the Account's SMTP server; a
copy goes to Sent (but on Gmail, which keeps its own), and what it replies to is flagged answered. `<Space>p`
(`:postpone`) postpones it to the Drafts Mailbox, where `:resume` resumes it, from the Index or the Pager; once it's
sent, or postponed again, the copy kept in Drafts goes.

## Reading a Message from a file

```sh
muttrs view message.eml
```

opens the Pager on a Message saved as a file, without any Account: its headers, then its HTML body as well as the
terminal can draw it (or its plain text, `<Space>b`), Inline Images included. Remote Content stays blocked until `I`
loads it, since loading it tells the sender you read the Message. `<Space>o` lists its Links to open (`<C-y>` yanks
one, as `gy` does the one at the cursor), `<Space>a` its Attachments to open (`<C-s>` saves one), `/` searches it, `:help` shows the keys,
and `q` quits.

## Configuration

`~/.config/muttrs/config.toml` (or under `$XDG_CONFIG_HOME`, on macOS too) says what's muttrs' own. Everything in it
is optional:

```toml
load_remote_content = false   # load Remote Content without being asked
prefer = "html"               # the body to show of a Message that has both: "html" or "plain"
pager_width = 100             # the widest bodies are laid out, in cells
pager_index_lines = 0         # rows of the Index kept above the Pager, as mutt's: a window of its own
pager_read_delay = 1          # seconds a Message shows before it's marked read; 0 at once, as NeoMutt's
max_picture_rows = 24         # the most rows a Picture takes
# pixel_ratio = 2.0           # the terminal's pixels to a CSS pixel; guessed from the cells' size otherwise
# download_dir = "~/Downloads"
# editor = "nvim"             # what Drafts are written in; $EDITOR, else $VISUAL, by default
threads = true                # the Index in threads (:threads switches), or newest first
hide_thread_subjects = false  # a reply's subject as an arrow, ├─>, where it's what it replies to's, as mutt does
voight = "cpu"                # where voight assesses the Pager's prose: "off", or "cuda", "metal", "wgpu" if built so
```

What's on screen is the kit's, and `~/.config/muttrs/init.vim` says it, after `~/.config/wcgw/init.vim`:
`:colorscheme` (tokyonight-moon by default; `default` is the terminal's own colours, which the `terminal` theme was),
`:set icons=auto|nerd|plain`, `:set graphics=auto|kitty|halfblocks|off` and `:set mouse=`, `:hi` for any group, and
`:map` for keys. muttrs' groups are named `Muttrs…` (`MuttrsUnread`, `MuttrsFlagged`, `MuttrsLabel`, …) and link to
the standard ones, with colours of their own in tokyonight-moon; `MuttrsPage` has the background authors' colours
are judged against. `theme`, `icons`, `images` and `mouse` in config.toml, and themes in `themes/`, are no longer
read: `scripts/convert-config.py` writes them as `:colorscheme`, `:set` and `:hi` lines.

muttrs keeps the Mailboxes, the Envelopes listed and the Messages read in `~/.cache/muttrs/accounts/`, readable by
you alone: they show at once when muttrs starts, before the server says what's new, and can be read offline. What's
listed is brought up to date by what changed (Messages new or gone, Flags, Labels), as a Message's headers never
change: where the server keeps mod-sequences (CONDSTORE, as Dovecot and Gmail do), only the Flags of the Messages
that changed since are asked for, else every listed one's; should it be kept wrong, `:sync` forgets what's kept of the open Mailbox and lists it anew,
as when it was first opened.

Logs go to `~/.local/state/muttrs/muttrs.log` (or under `$XDG_STATE_HOME`); `MUTTRS_LOG=debug` says more.

## License

Not chosen yet.
