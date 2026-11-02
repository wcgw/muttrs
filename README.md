# muttrs

A terminal mail client after mutt and vim, that renders HTML mail, images included, as well as a terminal can.

muttrs reads mail over IMAP and sends it over SMTP, each over TLS or STARTTLS, and signs in to Gmail with OAuth2. The
words used here, and in the code, are defined in [CONTEXT.md](CONTEXT.md); what's built and what's next is in
[docs/plan.md](docs/plan.md).

> Work in progress: reading and writing work; threads and server-side search are next. See the plan.

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
the keyring. Google lets the refresh tokens of apps still "in testing" last 7 days, after which `muttrs login gmail`
again, or publish the app. With an [app password](https://myaccount.google.com/apppasswords) instead, set
`auth = "password"`.

Gmail's labels are Mailboxes; deleting moves to its Trash, archiving (`a`) takes a Message out of the Inbox, and
what's sent isn't copied to Sent, as Gmail does that itself. `provider = "outlook"` does the same for Outlook and
Microsoft 365, with a client registered in Microsoft Entra (untested so far).

## Keys

They work as in mutt and vim. `?` shows the ones that apply.

| Where | Keys |
|---|---|
| Anywhere | `?` keys · `:q` `q` quit (or close the Pager) · `$` `:refresh` ask for what's new · `:cd NAME` open a Mailbox |
| Sidebar | `j` `k` `g` `G` move · `Enter` open · `Tab` `l` the Index · `\` show or hide |
| Index | `j` `k` `g` `G` `Ctrl-d` `Ctrl-u` move · `Enter` read · `/` `n` `N` search senders and subjects · `F` flag · `U` Unread or read · `d` delete (to Trash) · `a` archive · `Tab` `h` the sidebar |
| Pager | `j` `k` `Space` `b` `Ctrl-d` `Ctrl-u` `g` `G` scroll · `J` `K` the next Message, the previous · `/` `n` `N` search · `T` HTML or plain text · `I` load Remote Content · `o` Links · `v` Attachments · `F` `U` `d` `a` as in the Index · `q` back |
| Writing, in the Index or the Pager | `m` a new Message · `r` reply · `R` reply to all · `f` forward |
| A Draft | `y` send · `e` edit again · `a` attach a file · `D` detach the last · `P` postpone to Drafts · `q` `q` discard |

## Writing

`m`, `r`, `R` and `f` open a Draft in your editor (`editor` in the configuration, else `$VISUAL`, `$EDITOR` or `vi`)
as a file of headers, a blank line, then the body:

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

Its name ends in `.eml`, so that vim and Neovim edit it as mail. The body is Markdown, sent as it is (the plain text)
and as the HTML it makes, in which each new line breaks the line, as in plain text mail. `Attach:` lines attach
files. Saving and quitting shows the Draft as it will be sent, until `y` sends it through the Account's SMTP server;
a copy goes to Sent (but on Gmail, which keeps its own), and what it replies to is flagged answered. `P` postpones it
to the Drafts Mailbox.

## Reading a Message from a file

```sh
muttrs view message.eml
```

opens the Pager on a Message saved as a file, without any Account: its headers, then its HTML body as well as the
terminal can draw it (or its plain text, `T`), Inline Images included. Remote Content stays blocked until `I` loads
it, since loading it tells the sender you read the Message. `o` lists its Links to open or copy, `v` its Attachments
to open or save, `/` searches it, and `?` shows the keys.

## Configuration

`~/.config/muttrs/config.toml` (or under `$XDG_CONFIG_HOME`, on macOS too). Everything in it is optional:

```toml
theme = "tokyonight-moon"     # or "terminal", the terminal's own colours
icons = "auto"                # Nerd Font icons: true, false, or "auto" (in Ghostty, kitty and WezTerm)
images = "auto"               # Pictures: "kitty", "blocks" (half blocks), "off", or "auto" for the terminal's best
load_remote_content = false   # load Remote Content without being asked
prefer = "html"               # the body to show of a Message that has both: "html" or "plain"
pager_width = 100             # the widest bodies are laid out, in cells
max_picture_rows = 24         # the most rows a Picture takes
# pixel_ratio = 2.0           # the terminal's pixels to a CSS pixel; guessed from the cells' size otherwise
# download_dir = "~/Downloads"
# editor = "nvim"             # what Drafts are written in; $VISUAL, $EDITOR or vi by default
```

Logs go to `~/.local/state/muttrs/muttrs.log` (or under `$XDG_STATE_HOME`); `MUTTRS_LOG=debug` says more.

## License

Not chosen yet.
