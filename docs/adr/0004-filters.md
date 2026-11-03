---
status: accepted
---

# Filters are written as Gmail's search, evaluated on what the Index lists

`:select` and `:unselect` take a Filter: `qualifier:value` terms apart by spaces, as Gmail's search writes them, and
as ghd's filters do. `from:`, `to:` and `cc:` pick a Message one of whose senders, or recipients in that header, has
the value in their name or address, ignoring case; `subject:` one whose subject holds it (in double quotes if it has
spaces); `label:` one with the Label of that name, all of it, ignoring case; `is:` `unread`, `read`, `flagged`,
`answered` or `draft` by its Flags; `has:attachment` one that likely has Attachments, as the Index's mark says. `-` before a term leaves out what it picks.

A qualifier repeated picks any of its values (`from:ann from:bob`); different qualifiers, all of them (`from:ann
is:unread`); negated terms, none. As in ghd, `is:` counts as several qualifiers for that rule, one a Flag, so
`is:unread is:flagged` is both, and `is:unread is:read` either. A word that isn't `qualifier:value`, or a qualifier or
`is:` value muttrs doesn't know, is refused with what it does know, rather than a Filter that silently picks nothing;
`is:starred` says it's `is:flagged`.

A Filter is evaluated on the Envelopes the Index lists, folded or not, not on the server: it's instant, works offline,
and picks exactly what's on screen.

## Considered options

- **mutt's patterns** (`~f ann ~U`): what mutt users know, but terse and their own language; muttrs' `S` already
  takes Gmail's syntax on Gmail, and ghd, alongside, takes GitHub's, which is written the same way.
- **Searching the server** (`UID SEARCH FROM ann UNSEEN`): it would pick Messages not yet listed too, but the
  Selection is of the Index's rows, and the server's matching (on raw headers, by substring, encoded words or not)
  differs from server to server.

## Consequences

- Only Messages listed are picked: in a Mailbox with more than the 300 listed, older ones need listing first (the
  Index lists more as its cursor nears the end), or `S` to search the server.
- A Filter's terms each read one Envelope field, so qualifiers that need the body (text anywhere) or dates (`before:`,
  `after:`) aren't there yet; adding one means deciding how it repeats.
