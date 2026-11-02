//! Threads: Messages arranged by what they reply to, from their `Message-ID`, `References` and `In-Reply-To`, as
//! Jamie Zawinski's algorithm does it (without grouping by subject, which guesses wrong as often as right).
//!
//! A Message whose parent isn't listed (it's in another Mailbox, or older than what's listed) goes under the
//! nearest ancestor that is; when none is, the earliest of its siblings heads their thread.
//!
//! On Gmail, threads Gmail has as one (`X-GM-THRID`) are joined under the earliest of them, as when a reply drops
//! its References.

use std::collections::HashMap;

use jiff::Timestamp;

use crate::envelope::Envelope;

/// A Message's place in its thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Threaded {
    /// The Envelope's index in what was threaded.
    pub envelope: usize,
    /// How deep in its thread it is: 0 for the Message heading it.
    pub depth: usize,
    /// The tree drawn before it, one branch a level below the thread's head.
    pub branches: Vec<Branch>,
    /// The index, in what's returned, of the row heading its thread.
    pub head: usize,
    /// How many Messages follow in its thread, for the one heading it; 0 for the others.
    pub replies: usize,
}

/// A piece of the tree drawn before a Message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    /// An ancestor with more replies below: `│`.
    Through,
    /// An ancestor without: blank.
    Blank,
    /// The Message, with more replies to its parent after it: `├─`.
    Tee,
    /// The Message, its parent's last reply: `└─`.
    Corner,
}

/// A Message in the tree, and its replies.
struct Node {
    envelope: usize,
    children: Vec<Node>,
}

/// A Message-ID, or one listed only by what refers to it.
struct Container {
    envelope: Option<usize>,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// `envelopes` in threads: each thread's Messages oldest first under their parents, the threads newest first by
/// their latest Message.
#[must_use]
pub fn threads(envelopes: &[Envelope]) -> Vec<Threaded> {
    let mut containers: Vec<Container> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut container_of = |id: &str, containers: &mut Vec<Container>| -> usize {
        *by_id.entry(id.to_ascii_lowercase()).or_insert_with(|| {
            containers.push(Container {
                envelope: None,
                parent: None,
                children: Vec::new(),
            });
            containers.len() - 1
        })
    };
    let mut own: Vec<usize> = Vec::with_capacity(envelopes.len());
    for (index, envelope) in envelopes.iter().enumerate() {
        // A Message without an ID, or with one another has, gets a container of its own.
        let listed = envelope
            .message_id
            .as_deref()
            .map(|id| container_of(id, &mut containers))
            .filter(|&found| containers[found].envelope.is_none());
        let container = listed.unwrap_or_else(|| {
            containers.push(Container {
                envelope: None,
                parent: None,
                children: Vec::new(),
            });
            containers.len() - 1
        });
        containers[container].envelope = Some(index);
        own.push(container);
    }
    for (index, envelope) in envelopes.iter().enumerate() {
        let references = if envelope.references.is_empty() {
            &envelope.in_reply_to
        } else {
            &envelope.references
        };
        let chain: Vec<usize> = references
            .iter()
            .map(|id| container_of(id, &mut containers))
            .collect();
        // Each reference is a reply to the one before, unless something said otherwise already.
        for pair in chain.windows(2) {
            let (parent, child) = (pair[0], pair[1]);
            if containers[child].parent.is_none() {
                link(&mut containers, parent, child);
            }
        }
        // The Message itself replies to the last.
        if let Some(&parent) = chain.last() {
            let child = own[index];
            if let Some(previous) = containers[child].parent {
                containers[previous].children.retain(|&c| c != child);
                containers[child].parent = None;
            }
            link(&mut containers, parent, child);
        }
    }

    let date = |node: &Node| envelopes[node.envelope].date;
    let mut threads: Vec<Node> = Vec::new();
    for root in (0..containers.len()).filter(|&c| containers[c].parent.is_none()) {
        let mut nodes = build(&containers, root);
        if nodes.len() > 1 {
            // The parent heading them isn't listed: the earliest of them heads the thread.
            nodes.sort_by_key(|node| date(node));
            let mut head = nodes.remove(0);
            head.children.extend(nodes);
            threads.push(head);
        } else {
            threads.extend(nodes);
        }
    }
    let mut threads = join_gmail_threads(threads, envelopes, &date);
    for thread in &mut threads {
        sort(thread, &date);
    }
    let latest = |node: &Node| latest(node, &date);
    threads.sort_by_key(|thread| std::cmp::Reverse((latest(thread), thread.envelope)));

    let mut rows = Vec::with_capacity(envelopes.len());
    for thread in &threads {
        let head = rows.len();
        flatten(thread, 0, &[], head, &mut rows);
        rows[head].replies = rows.len() - head - 1;
    }
    rows
}

/// Makes `child` a reply to `parent`, unless it would make a loop.
fn link(containers: &mut [Container], parent: usize, child: usize) {
    let mut ancestor = Some(parent);
    while let Some(at) = ancestor {
        if at == child {
            return;
        }
        ancestor = containers[at].parent;
    }
    containers[child].parent = Some(parent);
    containers[parent].children.push(child);
}

/// `threads`, those in the same Gmail Thread, by any of their Messages, joined under the earliest of them.
fn join_gmail_threads(
    threads: Vec<Node>,
    envelopes: &[Envelope],
    date: &impl Fn(&Node) -> Option<Timestamp>,
) -> Vec<Node> {
    if envelopes
        .iter()
        .all(|envelope| envelope.gmail_thread.is_none())
    {
        return threads;
    }
    // Each thread's group, as the first thread of it (union-find): the threads sharing a Gmail Thread are one
    // group, and so are those sharing one with any of them.
    let mut group: Vec<usize> = (0..threads.len()).collect();
    let mut first_in: HashMap<u64, usize> = HashMap::new();
    for (at, thread) in threads.iter().enumerate() {
        let mut ids = Vec::new();
        gmail_threads(thread, envelopes, &mut ids);
        for id in ids {
            let other = *first_in.entry(id).or_insert(at);
            let (one, another) = (find(&mut group, at), find(&mut group, other));
            group[one.max(another)] = one.min(another);
        }
    }
    let mut joined: Vec<Vec<Node>> = (0..threads.len()).map(|_| Vec::new()).collect();
    for (at, thread) in threads.into_iter().enumerate() {
        joined[find(&mut group, at)].push(thread);
    }
    joined
        .into_iter()
        .filter(|nodes| !nodes.is_empty())
        .map(|mut nodes| {
            nodes.sort_by_key(|node| (date(node), node.envelope));
            let mut head = nodes.remove(0);
            head.children.extend(nodes);
            head
        })
        .collect()
}

/// The Gmail Threads of `node` and its replies.
fn gmail_threads(node: &Node, envelopes: &[Envelope], ids: &mut Vec<u64>) {
    ids.extend(envelopes[node.envelope].gmail_thread);
    for child in &node.children {
        gmail_threads(child, envelopes, ids);
    }
}

/// The group `at` is in: the first of it.
fn find(group: &mut [usize], mut at: usize) -> usize {
    while group[at] != at {
        group[at] = group[group[at]];
        at = group[at];
    }
    at
}

/// The Messages `container` stands for: itself with its replies, or, when it isn't listed, its replies in its place.
fn build(containers: &[Container], container: usize) -> Vec<Node> {
    let children: Vec<Node> = containers[container]
        .children
        .iter()
        .flat_map(|&child| build(containers, child))
        .collect();
    match containers[container].envelope {
        Some(envelope) => vec![Node { envelope, children }],
        None => children,
    }
}

fn sort(node: &mut Node, date: &impl Fn(&Node) -> Option<Timestamp>) {
    node.children
        .sort_by_key(|child| (date(child), child.envelope));
    for child in &mut node.children {
        sort(child, date);
    }
}

fn latest(node: &Node, date: &impl Fn(&Node) -> Option<Timestamp>) -> Option<Timestamp> {
    node.children
        .iter()
        .map(|child| latest(child, date))
        .fold(date(node), Option::max)
}

/// `node` and its replies as rows, after the branches of its ancestors' levels.
fn flatten(node: &Node, depth: usize, ancestors: &[Branch], head: usize, rows: &mut Vec<Threaded>) {
    rows.push(Threaded {
        envelope: node.envelope,
        depth,
        branches: ancestors.to_vec(),
        head,
        replies: 0,
    });
    for (index, child) in node.children.iter().enumerate() {
        let last = index + 1 == node.children.len();
        let mut branches = ancestors.to_vec();
        // What this level is for the levels below: a line through, if more replies follow.
        if let Some(own) = branches.last_mut() {
            *own = match *own {
                Branch::Tee => Branch::Through,
                Branch::Corner => Branch::Blank,
                other => other,
            };
        }
        branches.push(if last { Branch::Corner } else { Branch::Tee });
        flatten(child, depth + 1, &branches, head, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Flags;

    fn envelope(uid: u32, minute: i64, id: &str, references: &[&str]) -> Envelope {
        Envelope {
            uid,
            flags: Flags::default(),
            date: Some(Timestamp::from_second(1_800_000_000 + minute * 60).unwrap()),
            from: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            subject: format!("message {uid}"),
            message_id: Some(id.into()),
            in_reply_to: references
                .last()
                .map(|r| (*r).to_owned())
                .into_iter()
                .collect(),
            references: references.iter().map(|r| (*r).to_owned()).collect(),
            size: 0,
            has_attachments: false,
            gmail_thread: None,
        }
    }

    /// Each row as its UID after its tree.
    fn drawn(envelopes: &[Envelope]) -> Vec<String> {
        threads(envelopes)
            .iter()
            .map(|row| {
                let tree: String = row
                    .branches
                    .iter()
                    .map(|branch| match branch {
                        Branch::Through => "│ ",
                        Branch::Blank => "  ",
                        Branch::Tee => "├─",
                        Branch::Corner => "└─",
                    })
                    .collect();
                format!("{tree}{}", envelopes[row.envelope].uid)
            })
            .collect()
    }

    #[test]
    fn replies_go_under_what_they_reply_to_and_threads_by_their_latest() {
        let envelopes = [
            envelope(1, 0, "a", &[]),
            envelope(2, 1, "b", &["a"]),
            envelope(3, 2, "c", &["a", "b"]),
            envelope(4, 3, "d", &["a"]),
            envelope(5, 10, "e", &[]),
            envelope(6, 20, "f", &["a", "b", "c"]),
        ];
        // Thread a's latest is f (minute 20), after e (minute 10): it comes first.
        assert_eq!(
            drawn(&envelopes),
            ["1", "├─2", "│ └─3", "│   └─6", "└─4", "5"]
        );
        let rows = threads(&envelopes);
        assert_eq!((rows[0].replies, rows[3].depth, rows[3].head), (4, 3, 0));
    }

    #[test]
    fn a_missing_parent_leaves_its_replies_together() {
        // a isn't listed: b and c reply to it, d to b.
        let envelopes = [
            envelope(2, 1, "b", &["a"]),
            envelope(3, 2, "c", &["a"]),
            envelope(4, 3, "d", &["a", "b"]),
        ];
        assert_eq!(
            drawn(&envelopes),
            ["2", "├─3", "└─4"],
            "by date under the earliest"
        );
    }

    fn on_gmail(thread: u64, envelope: Envelope) -> Envelope {
        Envelope {
            gmail_thread: Some(thread),
            ..envelope
        }
    }

    #[test]
    fn gmail_joins_threads_references_dont() {
        let envelopes = [
            on_gmail(7, envelope(1, 0, "a", &[])),
            // A reply that dropped its References, say.
            on_gmail(7, envelope(2, 5, "b", &[])),
            on_gmail(7, envelope(3, 2, "c", &["a"])),
            on_gmail(8, envelope(4, 3, "d", &[])),
            envelope(5, 4, "e", &[]),
        ];
        assert_eq!(
            drawn(&envelopes),
            ["1", "├─3", "└─2", "5", "4"],
            "b under a, by date; d and e alone"
        );
        assert_eq!(threads(&envelopes)[0].replies, 2);
    }

    #[test]
    fn a_thread_in_two_gmail_threads_joins_both() {
        let envelopes = [
            on_gmail(2, envelope(1, 0, "z", &[])),
            on_gmail(1, envelope(2, 1, "x", &[])),
            // y replies to x, but Gmail has it with z.
            on_gmail(2, envelope(3, 2, "y", &["x"])),
            on_gmail(1, envelope(4, 3, "w", &[])),
        ];
        assert_eq!(drawn(&envelopes), ["1", "├─2", "│ └─3", "└─4"]);
    }

    #[test]
    fn loops_and_duplicates_dont_break_it() {
        let envelopes = [
            envelope(1, 0, "a", &["b"]),
            envelope(2, 1, "b", &["a"]),
            envelope(3, 2, "a", &[]),
            Envelope {
                message_id: None,
                ..envelope(4, 3, "", &[])
            },
        ];
        let rows = threads(&envelopes);
        assert_eq!(rows.len(), 4, "every Message once: {:?}", drawn(&envelopes));
    }
}
