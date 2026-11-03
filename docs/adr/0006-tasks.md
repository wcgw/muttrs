---
status: accepted
---

# What's asked of an Account is a Task, shown done until its worker says it's finished

An Account's worker has one IMAP session, and does what the UI asks of it one Request at a time: flagging, moving,
labelling, opening a Mailbox, fetching a Message, sending. The UI shows what it asked done at once (a Message
archived leaves the Index), before the server's done it. Each Request done tells the UI the Mailbox's Envelopes as the
server has them since; told while later Requests still wait, they showed those undone again: archiving three Messages
quickly, the second came back once the first was archived, until it was archived in turn. And sending held the Draft
on the screen, every key ignored, until the server answered.

So each Request asked is a **Task**, numbered by the Account's `Client` (`send` returns it). The worker tells
`Update::Started` when it starts on one and `Update::Finished` once it's done with it, whether the server did what was
asked or refused; then, for a Task that changes a Mailbox's Messages, the Mailbox's Envelopes again, as they stand. The
UI keeps each Account's Tasks not finished with what they change (Messages taken out of a Mailbox, a Flag or a Label
set or taken off), and shows every Envelope it's told, from the cache too, with those changes made: what the server
tells meanwhile is from before them. Once a Task's finished, the Envelopes told after it show what the server did,
what it refused too: a Message the server wouldn't archive comes back then.

Tasks are done in the order asked, but for fetching a Message to read, which goes before those waiting: it changes
nothing, and the Pager waits for it. Sending is a Task too, done beside the others (SMTP isn't the IMAP session): `y`
puts the Draft away at once; sent, it's done with; not sent, it's shown again with why.

The status line says the Task being done, once one's taken 300 ms (what's quicker would only flicker there), at once
for those that show nothing else while they're done (asking for what's new, searching, sending): a spinner, what it
does, how long once it's taken 3 seconds, and how many more are asked. Quitting with Tasks left waits for them, unless
a key's pressed first; `:q!` doesn't.

## Considered options

- **A second IMAP session for reading**, so that a Message to read never waits for a Task: one more connection per
  Account (Gmail allows 15, the Bridge has its own limits), two sessions to keep in step on the same Mailbox, and
  reading first in the queue covers what's quick. A long Task (the first listing of the Labels' Messages) still makes
  reading wait.
- **The UI keeping what the server told aside**, drawing it with the Tasks' changes, so that the worker needn't tell
  the Envelopes again once a Task's finished: a second copy of every listed Mailbox in the UI, to save telling one
  again from memory.
- **The worker skipping the refresh** while more is asked of the same Mailbox: fewer round trips when much is asked
  quickly, but what the server changed meanwhile shows only once the queue's empty. It may come later, as a speed-up:
  the Tasks' changes make it safe.

## Consequences

- A Task the connection dropped under is told finished, and isn't done again once connected: what it changed shows as
  the server has it. Those that waited are done once connected again.
- Offline, what would change the server is refused at once, and what the UI showed changed comes back.
- What the worker asks of itself (the copy kept in Sent, the answered Flag, once a Message is sent) isn't a Task the
  UI knows of, and the status line doesn't say it.
- A Draft that couldn't be sent is shown again once nothing else is written, typed or chosen: never over another.
