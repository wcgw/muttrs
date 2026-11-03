---
status: accepted
---

# Labels are Mailboxes a Message is in too: Gmail's by X-GM-LABELS, others' found by Message-ID

A Label is a Mailbox a Message can be in as well as the one it's listed in. The Index shows a Message's Labels, `Y`
adds or takes them off, and `:select label:NAME` picks by them. Which Mailboxes are Labels, and how a Message's are
known, depends on the server:

- **Gmail** (it has `X-GM-EXT-1`): its labels but its own (`[Gmail]/…`, the Inbox). Each Message's come with its
  Envelope, `X-GM-LABELS` fetched with `X-GM-THRID`, and again for those listed each time they're refreshed, as Flags
  are; `UID STORE ±X-GM-LABELS` adds or takes one off.
- **Elsewhere**: the Mailboxes under `Labels`, as Proton Mail's Bridge keeps them (`Labels/GH`). A Message has a Label
  if the Label's Mailbox has a Message with its `Message-ID`. Adding one copies the Messages that lack it there;
  taking it off expunges their copies there, found by `Message-ID`. On the Bridge, a copy to a Label's Mailbox
  labels the Message, and expunging from one only takes the label off: it deletes for good only from Trash and
  Drafts (its `RemoveMessagesFromMailbox` and `AddMessagesToMailbox`, in `internal/services/imapservice/connector.go`).
  The worker keeps, per Label, its Messages' `Message-ID`s by UID. Each time a Mailbox is opened (as when new mail
  comes to the open Inbox, and every five minutes), once its Envelopes are told, STATUS says which Labels' Mailboxes
  changed, and only those are examined again: `UID SEARCH ALL`, then the `Message-ID`s of the UIDs it hasn't seen.
  Until every Label's are known, the Envelopes keep the Labels the cache has for them.

## Considered options

- **IMAP keywords** (`$label1`, user keywords, as Thunderbird's tags): the protocol's own way, and cheap (they're
  Flags, fetched with the Envelope), but neither Gmail nor the Bridge shows them as labels, and many servers don't
  keep them. They'd be a third model to keep in step with the other two.
- **Searching each Label's Mailbox** for the `Message-ID`s listed (`UID SEARCH OR HEADER MESSAGE-ID …`): no state to
  keep, but a search per Label per refresh, each with up to 300 terms, which servers answer by scanning headers.
- **Proton's API**: what Proton's own apps use, but muttrs reads mail over IMAP only.

## Consequences

- Where Labels are Mailboxes of their own, a Message without a `Message-ID` shows none, and can't have one taken off.
- What's known of each Label's Mailbox isn't cached: the first time a session opens a Mailbox, every Label's
  `Message-ID`s are listed, which takes a while for a Label of tens of thousands of Messages.
- A Message whose copy in a Label's Mailbox has another `Message-ID` (rewritten by the server) isn't seen there.
- Labels are found by their Mailbox's name: a server with a `Labels` Mailbox of another meaning has its children
  taken for Labels, which then adds copies to them; that's what labelling there means anyway.
