//! What's asked of an Account and not done yet: its Tasks, done by its worker one at a time. The status line says
//! which is being done and how many more wait; and what they change of a Mailbox's Messages shows done already, on
//! the Envelopes the server tells too, until they're finished: what it tells meanwhile is from before them.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use muttrs_mail::client::{Client, Flag, Request, Task, Verdict};
use muttrs_mail::{Envelope, Flags, Label, Mailbox, Role};

/// How long a Task is waited for before the status line says it: what's done quicker would only flicker there. What
/// shows nothing else while it's done is said at once.
const UNSEEN: Duration = Duration::from_millis(300);

/// An Account's Tasks not finished, in the order asked.
#[derive(Debug, Default)]
pub(crate) struct Tasks {
    queued: Vec<Queued>,
}

/// A Task asked, not finished yet.
#[derive(Debug)]
struct Queued {
    task: Task,
    /// What it does, for people: `Archiving 3 Messages`.
    what: String,
    effect: Effect,
    /// Whether the status line says it at once: it shows nothing else while it's done.
    at_once: bool,
    asked: Instant,
    /// When the worker started on it.
    started: Option<Instant>,
}

/// What a Task changes of a Mailbox's Messages, shown before it's done.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    None,
    /// Takes them out of it: moving, deleting, archiving.
    Removes {
        mailbox: String,
        uids: HashSet<u32>,
    },
    Flags {
        mailbox: String,
        uids: HashSet<u32>,
        flag: Flag,
        on: bool,
    },
    Labels {
        mailbox: String,
        uids: HashSet<u32>,
        /// The Label's name.
        name: String,
        on: bool,
    },
}

/// What the status line says of an Account's Tasks.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Said<'a> {
    /// The Task being done, or, while none is, the next.
    pub(crate) what: &'a str,
    /// Since when it's been done; None while it waits.
    pub(crate) since: Option<Instant>,
}

impl Tasks {
    /// Asks `client`, of an Account with `mailboxes` and `labels`, for `request`, keeping the Task until it's
    /// finished: said as `what`, else as the request says.
    pub(crate) fn ask(
        &mut self,
        client: &Client,
        request: Request,
        what: Option<String>,
        (mailboxes, labels): (&[Mailbox], &[Label]),
        now: Instant,
    ) -> Task {
        let what = what.unwrap_or_else(|| describe(&request, mailboxes, labels));
        let effect = effect(&request, labels);
        let at_once = matches!(
            request,
            Request::Refresh
                | Request::Resync(_)
                | Request::Search { .. }
                | Request::SearchAll { .. }
                | Request::Send { .. }
                | Request::Bounce { .. }
                | Request::CreateMailbox(_)
                | Request::RenameMailbox { .. }
                | Request::DeleteMailbox(_)
        );
        let task = client.send(request);
        self.queued.push(Queued {
            task,
            what,
            effect,
            at_once,
            asked: now,
            started: None,
        });
        task
    }

    pub(crate) fn started(&mut self, task: Task, now: Instant) {
        if let Some(queued) = self.queued.iter_mut().find(|queued| queued.task == task) {
            queued.started = Some(now);
        }
    }

    pub(crate) fn finished(&mut self, task: Task) {
        self.queued.retain(|queued| queued.task != task);
    }

    pub(crate) fn len(&self) -> usize {
        self.queued.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// Shows `envelopes`, of `mailbox`, as they'll be once the Tasks are done.
    pub(crate) fn apply(&self, mailbox: &str, envelopes: &mut Vec<Envelope>) {
        for queued in &self.queued {
            match &queued.effect {
                Effect::Removes { mailbox: of, uids } if of == mailbox => {
                    envelopes.retain(|envelope| !uids.contains(&envelope.uid));
                }
                Effect::Flags {
                    mailbox: of,
                    uids,
                    flag,
                    on,
                } if of == mailbox => {
                    for envelope in envelopes.iter_mut().filter(|e| uids.contains(&e.uid)) {
                        set_flag(&mut envelope.flags, *flag, *on);
                    }
                }
                Effect::Labels {
                    mailbox: of,
                    uids,
                    name,
                    on,
                } if of == mailbox => {
                    for envelope in envelopes.iter_mut().filter(|e| uids.contains(&e.uid)) {
                        set_label(&mut envelope.labels, name, *on);
                    }
                }
                _ => {}
            }
        }
    }

    /// What the status line says `now`: the Task being done (the one started first) or the next, unless none has
    /// waited long enough to be worth saying.
    pub(crate) fn said(&self, now: Instant) -> Option<Said<'_>> {
        let worth = self
            .queued
            .iter()
            .any(|queued| queued.at_once || now.saturating_duration_since(queued.asked) >= UNSEEN);
        if !worth {
            return None;
        }
        let shown = self
            .queued
            .iter()
            .filter(|queued| queued.started.is_some())
            .min_by_key(|queued| queued.started)
            .or_else(|| self.queued.first())?;
        Some(Said {
            what: &shown.what,
            since: shown.started,
        })
    }
}

/// Sets `flag` on `flags`, or takes it off.
pub(crate) fn set_flag(flags: &mut Flags, flag: Flag, on: bool) {
    match flag {
        Flag::Seen => flags.seen = on,
        Flag::Flagged => flags.flagged = on,
        Flag::Answered => flags.answered = on,
    }
}

/// Adds the Label `name` to `labels`, in their order, or takes it off.
pub(crate) fn set_label(labels: &mut Vec<String>, name: &str, on: bool) {
    labels.retain(|label| label != name);
    if on {
        labels.push(name.to_owned());
        labels.sort_by(|a, b| muttrs_mail::label::order(a, b));
    }
}

/// What `request` changes of a Mailbox's Messages.
fn effect(request: &Request, labels: &[Label]) -> Effect {
    let set = |uids: &[u32]| uids.iter().copied().collect();
    match request {
        Request::Judge { mailbox, uids, .. }
        | Request::Move { mailbox, uids, .. }
        | Request::Expunge { mailbox, uids }
        | Request::Delete { mailbox, uids }
        | Request::Archive { mailbox, uids } => Effect::Removes {
            mailbox: mailbox.clone(),
            uids: set(uids),
        },
        Request::Flag {
            mailbox,
            uids,
            flag,
            on,
        } => Effect::Flags {
            mailbox: mailbox.clone(),
            uids: set(uids),
            flag: *flag,
            on: *on,
        },
        Request::Label {
            mailbox,
            uids,
            label,
            on,
        } => match labels.iter().find(|known| known.mailbox == *label) {
            Some(known) => Effect::Labels {
                mailbox: mailbox.clone(),
                uids: set(uids),
                name: known.name.clone(),
                on: *on,
            },
            None => Effect::None,
        },
        Request::Refresh
        | Request::Resync(_)
        | Request::Open(_)
        | Request::More(_)
        | Request::Search { .. }
        | Request::SearchAll { .. }
        | Request::Body { .. }
        | Request::MoveBack { .. }
        | Request::CreateMailbox(_)
        | Request::RenameMailbox { .. }
        | Request::DeleteMailbox(_)
        | Request::Bounce { .. }
        | Request::Append { .. }
        | Request::Send { .. } => Effect::None,
    }
}

/// What `request` does, for people, of an Account with `mailboxes` and `labels`.
fn describe(request: &Request, mailboxes: &[Mailbox], labels: &[Label]) -> String {
    let named = |name: &str| {
        mailboxes
            .iter()
            .find(|mailbox| mailbox.name == name)
            .map_or_else(
                || muttrs_mail::mailbox::decode(name),
                |m| m.label().to_owned(),
            )
    };
    let with_role = |role: Role| {
        mailboxes
            .iter()
            .find(|mailbox| mailbox.role == Some(role))
            .map_or_else(|| format!("{role:?}"), |m| m.label().to_owned())
    };
    match request {
        Request::Refresh => "Asking for what's new".to_owned(),
        Request::Resync(mailbox) => format!("Listing {} anew", named(mailbox)),
        Request::Open(mailbox) => format!("Opening {}", named(mailbox)),
        Request::More(mailbox) => format!("Listing more of {}", named(mailbox)),
        Request::Search { mailbox, query } => format!("Searching {} for {query}", named(mailbox)),
        Request::SearchAll { query } => format!("Searching every Mailbox for {query}"),
        Request::Body { .. } => "Fetching the Message".to_owned(),
        Request::Flag { uids, flag, on, .. } => {
            let messages = count(uids.len());
            match (flag, on) {
                (Flag::Seen, true) => format!("Marking {messages} read"),
                (Flag::Seen, false) => format!("Marking {messages} Unread"),
                (Flag::Flagged, true) => format!("Flagging {messages}"),
                (Flag::Flagged, false) => format!("Unflagging {messages}"),
                (Flag::Answered, _) => format!("Marking {messages} answered"),
            }
        }
        Request::Label {
            uids, label, on, ..
        } => {
            let name = labels
                .iter()
                .find(|known| known.mailbox == *label)
                .map_or(label.as_str(), |known| known.name.as_str());
            if *on {
                format!("Labelling {} {name}", count(uids.len()))
            } else {
                format!("Taking {name} off {}", count(uids.len()))
            }
        }
        Request::Judge { uids, verdict, .. } => {
            let as_what = match verdict {
                Verdict::Junk => "as spam",
                Verdict::NotJunk => "as not spam",
            };
            format!(
                "Moving {} to {}, {as_what}",
                count(uids.len()),
                with_role(verdict.role())
            )
        }
        Request::Move { uids, to, .. } => format!("Moving {} to {}", count(uids.len()), named(to)),
        Request::Expunge { uids, .. } => format!("Expunging {}", count(uids.len())),
        Request::Delete { mailbox, uids } => {
            let messages = count(uids.len());
            if mailboxes
                .iter()
                .any(|m| m.name == *mailbox && m.role == Some(Role::Trash))
            {
                format!("Deleting {messages} for good")
            } else {
                format!("Deleting {messages}")
            }
        }
        Request::Archive { uids, .. } => format!("Archiving {}", count(uids.len())),
        Request::MoveBack {
            to, message_ids, ..
        } => format!("Moving {} back to {}", count(message_ids.len()), named(to)),
        Request::Append { role, .. } => match role {
            Role::Drafts => "Postponing to Drafts".to_owned(),
            Role::Sent => "Keeping a copy in Sent".to_owned(),
            role => format!("Saving to {}", with_role(*role)),
        },
        Request::Send { .. } => "Sending".to_owned(),
        Request::Bounce { .. } => "Bouncing a Message".to_owned(),
        Request::CreateMailbox(path) => format!("Creating {path}"),
        Request::RenameMailbox { mailbox, to } => format!("Renaming {} {to}", named(mailbox)),
        Request::DeleteMailbox(mailbox) => format!("Deleting {}", named(mailbox)),
    }
}

/// `messages` Messages, for people.
fn count(messages: usize) -> String {
    match messages {
        1 => "a Message".to_owned(),
        messages => format!("{messages} Messages"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(uid: u32) -> Envelope {
        Envelope::read(uid, Flags::default(), None, 0, b"")
    }

    fn uids(envelopes: &[Envelope]) -> Vec<u32> {
        envelopes.iter().map(|envelope| envelope.uid).collect()
    }

    #[test]
    fn what_the_server_tells_shows_as_it_will_be_once_the_tasks_are_done() {
        let (client, _asked) = Client::detached();
        let gh = Label {
            name: "GH".into(),
            mailbox: "Labels/GH".into(),
        };
        let of = (&[][..], std::slice::from_ref(&gh));
        let now = Instant::now();
        let mut tasks = Tasks::default();
        let mut ask = |request| tasks.ask(&client, request, None, of, now);
        let archive = ask(Request::Archive {
            mailbox: "INBOX".into(),
            uids: vec![2],
        });
        let flag = ask(Request::Flag {
            mailbox: "INBOX".into(),
            uids: vec![1, 3],
            flag: Flag::Flagged,
            on: true,
        });
        let label = ask(Request::Label {
            mailbox: "INBOX".into(),
            uids: vec![3],
            label: "Labels/GH".into(),
            on: true,
        });
        ask(Request::Delete {
            mailbox: "Work".into(),
            uids: vec![1],
        });

        // As told by a refresh from before them: 2 is still there, nothing flagged.
        let mut told = vec![envelope(1), envelope(2), envelope(3)];
        tasks.apply("INBOX", &mut told);
        assert_eq!(uids(&told), [1, 3], "archived already, and 1 isn't Work's");
        assert!(told.iter().all(|envelope| envelope.flags.flagged));
        assert_eq!(told[1].labels, ["GH"]);

        // Once they're done, what's told is as it is: archiving refused, 2 is back.
        for task in [archive, flag, label] {
            tasks.finished(task);
        }
        let mut told = vec![envelope(1), envelope(2), envelope(3)];
        tasks.apply("INBOX", &mut told);
        assert_eq!(uids(&told), [1, 2, 3]);
        assert!(!told[0].flags.flagged);
        assert_eq!(tasks.len(), 1, "Work's deletion");
    }

    #[test]
    fn the_status_line_says_the_task_being_done_once_it_takes_a_while_and_how_many_more() {
        let (client, _asked) = Client::detached();
        let mailboxes = [Mailbox::listed("INBOX", Some("/"), &[])];
        let of = (&mailboxes[..], &[][..]);
        let now = Instant::now();
        let mut tasks = Tasks::default();
        let open = tasks.ask(&client, Request::Open("INBOX".into()), None, of, now);
        assert_eq!(tasks.said(now), None, "quick, as most are: not said");
        let later = now + UNSEEN;
        assert_eq!(
            tasks.said(later),
            Some(Said {
                what: "Opening Inbox",
                since: None,
            }),
            "waiting"
        );
        let archive = tasks.ask(
            &client,
            Request::Archive {
                mailbox: "INBOX".into(),
                uids: vec![1, 2],
            },
            None,
            of,
            later,
        );
        tasks.started(archive, later);
        assert_eq!(
            tasks.said(later),
            Some(Said {
                what: "Archiving 2 Messages",
                since: Some(later),
            }),
            "the one started, before the first asked"
        );
        assert_eq!(tasks.len(), 2);
        tasks.finished(open);
        tasks.finished(archive);
        let refresh = tasks.ask(&client, Request::Refresh, None, of, later);
        assert_eq!(
            tasks.said(later).map(|said| said.what),
            Some("Asking for what's new"),
            "at once: nothing else shows it"
        );
        tasks.finished(refresh);
        assert!(tasks.is_empty());
    }

    #[test]
    fn labels_are_kept_in_order() {
        let mut labels = vec!["GH".to_owned(), "work".to_owned()];
        set_label(&mut labels, "Rust", true);
        assert_eq!(labels, ["GH", "Rust", "work"]);
        set_label(&mut labels, "GH", false);
        assert_eq!(labels, ["Rust", "work"]);
    }
}
