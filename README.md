# muttrs

A terminal mail client after mutt and vim, that renders HTML mail, images included, as well as a terminal can.

muttrs reads mail over IMAP and sends it over SMTP, each over TLS or STARTTLS, and signs in to Gmail with OAuth2. The
words used here, and in the code, are defined in [CONTEXT.md](CONTEXT.md); what's built and what's next is in
[docs/plan.md](docs/plan.md).

> Work in progress: see the plan for what works yet.

## Requirements

- Linux or macOS, and Rust 1.88 or newer to build it.
- For Pictures at full resolution, a terminal with kitty's Unicode placeholders: [Ghostty](https://ghostty.org) or
  [kitty](https://sw.kovidgoyal.net/kitty/). Any other terminal draws them with half blocks.
- For icons, a [Nerd Font](https://www.nerdfonts.com). Ghostty, kitty and WezTerm have the symbols built in.

## Install

```sh
cargo install --locked --path crates/muttrs
```

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
```

Logs go to `~/.local/state/muttrs/muttrs.log` (or under `$XDG_STATE_HOME`); `MUTTRS_LOG=debug` says more.

## License

Not chosen yet.
