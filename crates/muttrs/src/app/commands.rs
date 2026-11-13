//! muttrs' ex commands and default keys (wcgw-kit's D12, D13): every action on Messages, Mailboxes and Drafts has a
//! command, so `:map` can rebind anything a key does, and the keys follow the kit's shared verbs where mail has them
//! (`c` new, `r` reply, `s` flag, `dd` delete, `K` details, `gx` open a Link, `<C-l>` refresh).

use muttrs_mail::client::{Flag, Verdict};
use wcgw_kit::keymap::{Mode, Topic};
use wcgw_kit::shell::Command;

use super::{Kit, Muttrs, Writing};
use crate::index::Sort;

/// Reading and acting on Messages, in `:help`: between the kit's Opening and Buffers.
const MAIL: Topic = Topic::new(72, "Mail");
/// Writing, in `:help`.
const WRITING: Topic = Topic::new(74, "Writing");
/// The Index's order and Selection, in `:help`.
const LISTING: Topic = Topic::new(76, "Listing");

/// muttrs' default keys: keys, the command each runs, what which-key says of it, and its `:help` topic.
const KEYS: [(&str, &str, &str, Topic); 42] = [
    (
        "<2-LeftMouse>",
        "open",
        "open what's clicked",
        Topic::OPENING,
    ),
    (
        "K",
        "details",
        "details: every header, or a card",
        Topic::OPENING,
    ),
    (
        "gx",
        "browse",
        "open the Link in the browser",
        Topic::OPENING,
    ),
    ("gy", "yanklink", "copy the Link", Topic::OPENING),
    (
        "<leader>o",
        "pick links",
        "Links: <C-y> copies",
        Topic::OPENING,
    ),
    (
        "<leader>a",
        "pick attachments",
        "Attachments: <C-s> saves",
        Topic::OPENING,
    ),
    ("<C-n>", "next", "the next Message, older", MAIL),
    ("<C-p>", "previous", "the previous Message, newer", MAIL),
    ("I", "remote", "load Remote Content", MAIL),
    ("<leader>b", "body", "HTML or plain text", MAIL),
    ("s", "flag", "flag, or not", MAIL),
    ("U", "unread", "Unread, or read", MAIL),
    ("dd", "delete", "delete, to Trash", MAIL),
    ("a", "archive", "archive", MAIL),
    ("u", "undo", "move back what went last", MAIL),
    ("<leader>m", "pick mailboxes", "move to a Mailbox", MAIL),
    ("<leader>l", "pick labels", "Labels", MAIL),
    ("<leader>j", "junk", "to Junk, as spam", MAIL),
    ("<leader>J", "notjunk", "out of Junk, not spam", MAIL),
    ("|", "pipe", "pipe to a command", MAIL),
    ("!", "shell", "run a command", MAIL),
    ("\\", "sidebar", "show or hide the sidebar", Topic::WINDOWS),
    ("<F1>", "help", "help", Topic::SEARCHING),
    ("<leader>?", "help", "help", Topic::SEARCHING),
    (
        "<Esc>",
        "clear",
        "clear the Selection, or else the limit",
        LISTING,
    ),
    ("S", "grep", "search the server", LISTING),
    ("od", "sort date", "sort newest first", LISTING),
    ("of", "sort from", "sort by sender", LISTING),
    ("os", "sort subject", "sort by subject", LISTING),
    ("oz", "sort size", "sort largest first", LISTING),
    ("ot", "sort threads", "in threads", LISTING),
    ("Od", "sort! date", "sort oldest first", LISTING),
    ("Of", "sort! from", "sort by sender, Z to A", LISTING),
    ("Os", "sort! subject", "sort by subject, Z to A", LISTING),
    ("Oz", "sort! size", "sort smallest first", LISTING),
    ("c", "compose", "a new Message", WRITING),
    ("r", "reply", "reply", WRITING),
    ("R", "reply all", "reply to all", WRITING),
    (
        "<leader>L",
        "reply list",
        "reply to the mailing list",
        WRITING,
    ),
    ("<leader>f", "forward", "forward", WRITING),
    ("<leader>s", "send", "send the Draft", WRITING),
    ("<leader>p", "postpone", "postpone the Draft", WRITING),
];

/// muttrs' defaults that go on past a command: `x` selects and moves down, as ghd's `s`. Keys, the keys they stand
/// for, what which-key says of them, and their `:help` topic.
const MAPS: [(&str, &str, &str, Topic); 1] =
    [("x", ":toggle<CR>j", "select or not, and down", LISTING)];

/// Every argument is one of `choices`, or there's none.
fn one_of(
    choices: &'static [&'static str],
) -> impl Fn(&Muttrs, &Kit, &[&str], &str) -> Vec<String> {
    move |_, _, before, _| {
        if before.is_empty() {
            choices.iter().map(|&choice| choice.to_owned()).collect()
        } else {
            Vec::new()
        }
    }
}

/// Filters' words, as the Messages the Index lists would have them.
fn filters(muttrs: &Muttrs, kit: &Kit, _: &[&str], word: &str) -> Vec<String> {
    let listed = muttrs
        .current_index(kit)
        .map_or(&[][..], |(_, index)| index.envelopes());
    crate::filter::completions(word, listed)
}

/// Addresses that start with `word`, as seen or found by the query command.
fn addresses(muttrs: &Muttrs, _: &Kit, _: &[&str], word: &str) -> Vec<String> {
    muttrs.addresses_starting(word.trim_start_matches(','))
}

/// The Mailboxes of the Account shown, by their paths.
fn mailboxes(muttrs: &Muttrs, kit: &Kit, before: &[&str], _: &str) -> Vec<String> {
    if !before.is_empty() {
        return Vec::new();
    }
    let account = muttrs.account_shown(kit);
    muttrs.accounts.get(account).map_or_else(Vec::new, |state| {
        state
            .mailboxes
            .iter()
            .map(|mailbox| mailbox.path.join("/"))
            .collect()
    })
}

/// `args`, or why it's needed.
fn required(args: &str) -> Result<&str, String> {
    if args.is_empty() {
        Err("E471: Argument required".to_owned())
    } else {
        Ok(args)
    }
}

/// Adds muttrs' commands, pickers and default keys to `kit`.
#[allow(clippy::too_many_lines)] // A table of commands, each short.
pub(super) fn add(kit: &mut Kit) {
    let commands = [
        // Reading.
        Command::<Muttrs>::new("exit", 3, |muttrs, kit, _| Ok(muttrs.exit(kit))).help(
            "close the Pager, the Draft or what's found; else quit once what's asked is done",
        ),
        Command::new("next", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.step(kit, true)
        })
        .help("in the Pager, the next Message, older"),
        Command::new("previous", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.step(kit, false)
        })
        .help("in the Pager, the previous Message, newer"),
        Command::new("details", 5, |muttrs: &mut Muttrs, kit, _| {
            muttrs.details(kit).map(|()| Vec::new())
        })
        .help("in the Pager, every header as written, or those read; in the Index, the Envelope"),
        Command::new("raw", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle_raw(kit).map(|()| Vec::new())
        })
        .help("the Message as it came, or as read"),
        Command::new("body", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle_body(kit).map(|()| Vec::new())
        })
        .help("the Message's other body: HTML or plain text"),
        Command::new("remote", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.load_remote(kit)
        })
        .help("load the Message's Remote Content"),
        Command::new("browse", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.browse(kit, false)
        })
        .help("open the Link under the cursor; without one, list them"),
        Command::new("yanklink", 5, |muttrs: &mut Muttrs, kit, _| {
            muttrs.browse(kit, true)
        })
        .help("copy the Link under the cursor; without one, list them"),
        Command::new("unsubscribe", 5, |muttrs: &mut Muttrs, kit, _| {
            muttrs.unsubscribe(kit)
        })
        .help("unsubscribe from the Message's mailing list"),
        // Acting on Messages.
        Command::new("flag", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle(kit, Flag::Flagged)
        })
        .help("flag the Messages, or not"),
        Command::new("unread", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle(kit, Flag::Seen)
        })
        .help("mark the Messages Unread, or read"),
        Command::new("delete", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.remove(kit, false)
        })
        .help("delete the Messages, to Trash"),
        Command::new("archive", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.remove(kit, true)
        })
        .help("archive the Messages"),
        Command::new("junk", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.judge(kit, Verdict::Junk)
        })
        .help("move the Messages to Junk, as spam"),
        Command::new("notjunk", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.judge(kit, Verdict::NotJunk)
        })
        .help("move the Messages out of Junk to the Inbox, as not spam"),
        Command::new("move", 4, |muttrs: &mut Muttrs, kit, line| {
            match line.args {
                "" => {
                    let picker = muttrs.move_picker(kit);
                    Ok(kit.open_picker(muttrs, picker))
                }
                name => muttrs.move_named(kit, name),
            }
        })
        .help("move the Messages to a Mailbox; alone, choose one")
        .complete(mailboxes),
        Command::new("undo", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.undo(kit);
            Ok(Vec::new())
        })
        .help("move what was deleted, archived or moved last back"),
        Command::new("pipe", 4, |muttrs: &mut Muttrs, kit, line| {
            match line.args {
                "" => {
                    kit.prompt("|", false, |muttrs: &mut Muttrs, kit, command| {
                        muttrs.pipe(kit, command.trim()).unwrap_or_else(|why| {
                            kit.error(why);
                            Vec::new()
                        })
                    });
                    Ok(Vec::new())
                }
                command => muttrs.pipe(kit, command),
            }
        })
        .help("pipe the Message, or those selected as an mbox, to a command"),
        Command::new("shell", 2, |_: &mut Muttrs, kit, line| match line.args {
            "" => {
                kit.prompt("!", false, |_: &mut Muttrs, _, command| {
                    Muttrs::shell(command.trim())
                });
                Ok(Vec::new())
            }
            command => Ok(Muttrs::shell(command)),
        })
        .help("run a command with the terminal; alone, the shell"),
        Command::new("bounce", 4, |muttrs: &mut Muttrs, kit, line| {
            muttrs.bounce(kit, required(line.args)?)
        })
        .help("send the Message on, as it is, to TO")
        .complete(addresses),
        // Listing.
        Command::new("toggle", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle_selected(kit).map(|()| Vec::new())
        })
        .help("select the Message at the cursor, or not"),
        Command::new("select", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs.select_by(kit, true, line.args).map(|()| Vec::new())
        })
        .help("select what a filter picks (from:ann is:unread label:work …); alone, all")
        .complete(filters),
        Command::new("unselect", 5, |muttrs: &mut Muttrs, kit, line| {
            muttrs.select_by(kit, false, line.args).map(|()| Vec::new())
        })
        .help("unselect what a filter picks; alone, all")
        .complete(filters),
        Command::new("clear", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.clear(kit).map(|()| Vec::new())
        })
        .help("clear the Selection, or else the limit"),
        Command::new("limit", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs.limit_to(kit, line.args).map(|()| Vec::new())
        })
        .help("list only what a filter picks; alone, all")
        .complete(filters),
        Command::new("sort", 3, |muttrs: &mut Muttrs, kit, line| {
            let sort = Sort::named(if line.args.is_empty() {
                "date"
            } else {
                line.args
            })?;
            muttrs.sort(kit, sort, line.bang).map(|()| Vec::new())
        })
        .bang()
        .help("sort by date, from, subject, size, or in threads; ! the other way")
        .complete(one_of(&Sort::NAMES)),
        Command::new("threads", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle_threads(kit).map(|()| Vec::new())
        })
        .help("the Index in threads, or newest first"),
        Command::new("grep", 2, |muttrs: &mut Muttrs, kit, line| {
            match line.args {
                "" => {
                    kit.prompt(
                        "Search the server: ",
                        false,
                        |muttrs: &mut Muttrs, kit, query| {
                            if let Err(why) = muttrs.search_server(kit, query.trim()) {
                                kit.error(why);
                            }
                            Vec::new()
                        },
                    );
                    Ok(Vec::new())
                }
                query => muttrs.search_server(kit, query).map(|()| Vec::new()),
            }
        })
        .help("search the open Mailbox on the server (Gmail's syntax on Gmail)"),
        Command::new("search", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs.search_all(kit, required(line.args)?);
            Ok(Vec::new())
        })
        .help("search every Mailbox on the server, for where it finds Messages"),
        // Mailboxes.
        Command::new("cd", 2, |muttrs: &mut Muttrs, kit, line| {
            muttrs
                .change_mailbox(kit, required(line.args)?)
                .map(|()| Vec::new())
        })
        .help("open a Mailbox by name")
        .complete(mailboxes),
        Command::new("create", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs
                .create_mailbox(kit, required(line.args)?)
                .map(|()| Vec::new())
        })
        .help("create a Mailbox (Work/Reports)"),
        Command::new("rename", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs
                .rename_mailbox(kit, required(line.args)?)
                .map(|()| Vec::new())
        })
        .help("rename the open Mailbox"),
        Command::new("delmailbox", 4, |muttrs: &mut Muttrs, kit, line| {
            muttrs
                .delete_mailbox(kit, required(line.args)?)
                .map(|()| Vec::new())
        })
        .help("delete a Mailbox, and its Messages")
        .complete(mailboxes),
        Command::new("refresh", 3, |muttrs: &mut Muttrs, kit, line| {
            if line.bang {
                muttrs.resync(kit)?;
            } else {
                muttrs.refresh();
            }
            Ok(Vec::new())
        })
        .bang()
        .help("ask for what's new; ! lists the open Mailbox anew"),
        Command::new("sync", 2, |muttrs: &mut Muttrs, kit, line| {
            if line.bang {
                muttrs.resync(kit)?;
            } else {
                muttrs.refresh();
            }
            Ok(Vec::new())
        })
        .bang()
        .help("ask for what's new; ! lists the open Mailbox anew"),
        Command::new("login", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs.login(kit, line.args)
        })
        .help("sign in to an Account again, in the browser")
        .complete(|muttrs: &Muttrs, _, before, _| {
            if before.is_empty() {
                muttrs
                    .accounts
                    .iter()
                    .map(|state| state.name.clone())
                    .collect()
            } else {
                Vec::new()
            }
        }),
        Command::new("sidebar", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.toggle_sidebar(kit);
            Ok(Vec::new())
        })
        .help("show or hide the sidebar"),
        // Writing.
        Command::new("compose", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs.compose(kit, line.args)
        })
        .help("write a new Message, to whom it says, or asked")
        .complete(addresses),
        Command::new("reply", 3, |muttrs: &mut Muttrs, kit, line| {
            let writing = match line.args {
                "" => Writing::Reply,
                "all" => Writing::ReplyAll,
                "list" => Writing::ReplyList,
                other => return Err(format!("E475: Invalid argument: {other}")),
            };
            muttrs.write(kit, writing)
        })
        .help("reply; all, to all; list, to the mailing list")
        .complete(one_of(&["all", "list"])),
        Command::new("forward", 3, |muttrs: &mut Muttrs, kit, line| {
            let writing = match line.args {
                "" => Writing::Forward,
                "attached" => Writing::ForwardAttached,
                other => return Err(format!("E475: Invalid argument: {other}")),
            };
            muttrs.write(kit, writing)
        })
        .help("forward the Message, quoted; attached, as an Attachment")
        .complete(one_of(&["attached"])),
        Command::new("resume", 3, |muttrs: &mut Muttrs, kit, _| {
            muttrs.resume(kit)
        })
        .help("resume a Postponed Draft, in Drafts"),
        Command::new("draft", 3, |muttrs: &mut Muttrs, _, _| {
            if muttrs.compose.is_none() {
                return Err("No Draft is being written".to_owned());
            }
            Ok(muttrs.edit_draft())
        })
        .help("edit the Draft again"),
        Command::new("send", 3, |muttrs: &mut Muttrs, kit, _| muttrs.send(kit))
            .help("send the Draft"),
        Command::new("postpone", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.postpone(kit)
        })
        .help("postpone the Draft to Drafts"),
        Command::new("attach", 3, |muttrs: &mut Muttrs, kit, line| {
            muttrs.attach(kit, line.args)
        })
        .help("attach a file to the Draft"),
        Command::new("detach", 5, |muttrs: &mut Muttrs, _, _| {
            muttrs.detach().map(|()| Vec::new())
        })
        .help("take the Draft's last Attachment off"),
        Command::new("discard", 4, |muttrs: &mut Muttrs, kit, _| {
            muttrs.discard(kit);
            Ok(Vec::new())
        })
        .help("discard the Draft"),
    ];
    for command in commands {
        kit.add_command(command);
    }
    super::pickers::add(kit);
    let keymap = kit.keymap_mut();
    for (notation, command, description, topic) in KEYS {
        keymap.bind(&[Mode::Normal], notation, command, description, topic);
    }
    for (notation, keys, description, topic) in MAPS {
        keymap.bind_keys(&[Mode::Normal], notation, keys, description, topic);
    }
    // `q` closes the Pager, back to its Index, rather than its window; in the Index, it quits once what's asked
    // is done, rather than leaving the sidebar alone.
    keymap.bind(
        &[Mode::Normal],
        "q",
        "exit",
        "close, or quit",
        Topic::WINDOWS,
    );
}
