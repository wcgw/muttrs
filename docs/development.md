# Development

## Layout

See [plan.md](plan.md#architecture) for the crates and how they depend on each other.

## Checks

What CI runs, on Linux and macOS:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets     # pedantic, and every warning is an error in CI
cargo test --workspace
```

## Trying things out

- `cargo run -p muttrs-render --example render -- FILE.html [WIDTH]` prints a body's Rendering as text.
- `cargo run -p muttrs -- view FILE.eml` opens the Pager on a Message saved as a file.
- With `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `XDG_STATE_HOME` pointed at a scratch directory, muttrs runs with a
  configuration, cache and log of its own, leaving yours alone. `MUTTRS_LOG=debug` logs more.

## Mail servers to test against

`scripts/test-servers.sh up` starts two servers in containers (podman, or docker with `CONTAINER_ENGINE=docker`),
on the loopback address only:

| Server | Ports | Signing in |
|---|---|---|
| GreenMail | IMAP 3143, IMAPS 3993, SMTP 3025, SMTPS 3465 | any user, any password; XOAUTH2 with any token |
| Dovecot | IMAP with STARTTLS 31143, IMAPS 31993 | any user, the password `secret` |

Both present a certificate for `localhost` from a CA the script makes, in `target/test-servers/ca.pem`. Then:

```sh
MUTTRS_TEST_CA=$PWD/target/test-servers/ca.pem cargo test -p muttrs-mail --test servers -- --ignored
scripts/test-servers.sh down
```

The tests sign in as users of their own, so they can run again without starting the servers afresh. Dovecot lists
Sent, Drafts, Junk and Trash with their SPECIAL-USE Roles; GreenMail lists only the Inbox.

To use them from muttrs, give an Account the CA:

```toml
[[account]]
name = "dovecot"
email = "alex@localhost"
username = "alex"
password_command = "echo secret"
ca_file = "/path/to/muttrs/target/test-servers/ca.pem"
imap = { host = "localhost", port = 31143, tls = "starttls" }
smtp = { host = "localhost", port = 3465 }
```

What the servers don't cover: Gmail's and Outlook's own behaviour (labels as Mailboxes, OAuth2 against Google's and
Microsoft's endpoints), and XOAUTH2 in IMAP's continuation form, which Gmail also takes; GreenMail only takes it on
the command's line (SASL-IR), which is what muttrs sends first.

## Conventions

- Words from [CONTEXT.md](../CONTEXT.md), capitalised as nouns (Message, Mailbox, Envelope), in code and docs alike.
- Decisions that will be lived with go in [adr/](adr); the plan says what's done and what's next.
- Commits follow Conventional Commits, scoped by area: `feat(pager): …`, `fix(imap): …`.
