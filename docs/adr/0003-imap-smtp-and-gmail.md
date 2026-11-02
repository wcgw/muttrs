---
status: accepted
---

# Read over IMAP, send over SMTP, sign in to Gmail with OAuth2

An Account reads its mail over IMAP4rev1 and sends over SMTP submission, each over implicit TLS (`ssl`, ports 993 and
465) or STARTTLS (ports 143 and 587). Unencrypted connections are refused, except to the loopback address, for
testing. Mail stays on the server; muttrs caches the bodies it has fetched.

Authentication is, per Account, either:

- **a password** (LOGIN, or SMTP's PLAIN and LOGIN), read from the system keyring (`secret-tool` on Linux, `security`
  on macOS) or printed by a command of the user's choosing (`pass`, `op`, …);
- **OAuth2** (XOAUTH2 on both protocols). `muttrs login ACCOUNT` runs the authorization code flow with PKCE in the
  browser, redirecting to a loopback port, and keeps the refresh token in the keyring. muttrs refreshes access tokens
  as they expire.

Gmail needs OAuth2 unless the account allows app passwords, and Google wants each application to have its own OAuth
client: the user creates one ("Desktop app") in their Google Cloud project and gives muttrs its id and secret. muttrs
ships no client id of its own and never borrows another application's. Gmail has more quirks that the `gmail` preset
takes care of: its labels are Mailboxes; `[Gmail]` can't be selected; deleting means moving to its Trash (removing a
label otherwise); archiving means leaving the Inbox; and its SMTP server files sent mail itself, so muttrs doesn't
append a copy to Sent.

Mailboxes' Roles (Sent, Drafts, Trash, Junk, Archive, All) come from SPECIAL-USE (RFC 6154), which Gmail, Fastmail,
Outlook and Dovecot all report, and can be set in the configuration.

## Considered options

- **Maildir, synced by mbsync**, as many mutt users do: offline for free, but another tool to set up, and no IDLE.
- **JMAP**: better, but few providers have it, and not Gmail.
- **An app password only**: Google has been removing them, and Workspace admins often disable them.

## Consequences

- Each Account has a worker task owning its IMAP session; a second session IDLEs on the Inbox. Gmail drops IDLE after
  about 30 minutes, so it's renewed every 25.
- IMAP is async-imap over tokio-rustls; SMTP is lettre. Both use the platform's root certificates, plus a CA file
  when one is configured. A server with a certificate of its own (a mail bridge's, self-signed and marked a CA's,
  which webpki won't take as a server's) is trusted by naming that certificate instead (`server_certificate`, which
  `muttrs trust` saves): it must be exactly one of those, and nothing else is checked of it but that the handshake
  proves its key. lettre takes no verifier of muttrs' own, so SMTP connects without verifying, then compares the
  certificate presented before signing in; lettre's lenient verifier still checks the handshake's signatures.
- Outlook and Microsoft 365 work the same way, with Microsoft's endpoints and scopes, but are untested.
