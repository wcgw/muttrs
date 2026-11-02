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

## License

Not chosen yet.
