//! muttrs driven as the user meets it, through the kit's harness (wcgw-kit's D19): keys in, the screen and what's
//! asked of the Accounts out. No server: each Account's Client is detached, and what it would tell is sent by hand.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use jiff::civil::date;
use jiff::tz::TimeZone;
use muttrs_mail::client::{Client, Connection, Request, Task, Update};
use muttrs_mail::{Address, Envelope, Flags, Mailbox};
use tokio::sync::mpsc::UnboundedReceiver;
use unicode_width::UnicodeWidthStr as _;
use wcgw_kit::testing::Harness;

use super::{Effects, Io, Msg, Muttrs};
use crate::addresses::AddressBook;
use crate::config::Config;
use crate::sidebar::AccountState;

/// muttrs on one Account, `home`, whose Client is detached: what's asked of it waits in `asked`.
struct Mail {
    harness: Harness<Muttrs>,
    asked: UnboundedReceiver<(Task, Request)>,
}

fn mailboxes() -> Vec<Mailbox> {
    [
        ("INBOX", &[][..]),
        ("Archive", &["\\Archive"][..]),
        ("Drafts", &["\\Drafts"][..]),
        ("Trash", &["\\Trash"][..]),
        ("Work", &[][..]),
        ("Work/Reports", &[][..]),
    ]
    .iter()
    .map(|(name, attributes)| Mailbox::listed(name, Some("/"), attributes))
    .collect()
}

fn address(name: &str) -> Address {
    Address {
        name: Some(name.to_owned()),
        email: format!("{}@example.com", name.to_lowercase()),
    }
}

fn envelope(uid: u32, day: i8, subject: &str, from: &str) -> Envelope {
    Envelope {
        uid,
        flags: Flags::default(),
        date: Some(
            date(2026, 11, day)
                .at(9, 0, 0, 0)
                .to_zoned(TimeZone::UTC)
                .unwrap()
                .timestamp(),
        ),
        from: vec![address(from)],
        to: vec![address("Alex")],
        cc: Vec::new(),
        subject: subject.into(),
        message_id: Some(format!("{uid}@example.com")),
        in_reply_to: Vec::new(),
        references: Vec::new(),
        size: 1000,
        has_attachments: false,
        gmail_thread: None,
        labels: Vec::new(),
    }
}

fn reply(uid: u32, day: i8, to: &[u32]) -> Envelope {
    let references: Vec<String> = to.iter().map(|uid| format!("{uid}@example.com")).collect();
    Envelope {
        in_reply_to: references.last().cloned().into_iter().collect(),
        references,
        ..envelope(uid, day, "Re: Lunch", "Bob")
    }
}

/// A picture of 40 × 40 pixels, as PNG.
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(40, 40, image::Rgba([200, 30, 30, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// A Message with an HTML body showing an Inline Image, a Link, and an Attachment; and its plain text.
fn html_message(subject: &str) -> Vec<u8> {
    let picture = base64::engine::general_purpose::STANDARD.encode(png());
    format!(
        "From: Ann <ann@example.com>\r\nTo: Alex <alex@example.com>\r\nSubject: {subject}\r\n\
         Message-ID: <1@example.com>\r\nDate: Mon, 2 Nov 2026 09:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"outer\"\r\n\r\n\
         --outer\r\nContent-Type: multipart/related; boundary=\"related\"\r\n\r\n\
         --related\r\nContent-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
         --alt\r\nContent-Type: text/plain\r\n\r\nHello there, in plain text.\r\n\
         --alt\r\nContent-Type: text/html\r\n\r\n<p>Hello <a href=\"https://example.com/menu\">the menu</a>.</p>\
         <p><img src=\"cid:pic@example.com\" alt=\"a red square\"></p>\r\n--alt--\r\n\
         --related\r\nContent-Type: image/png\r\nContent-ID: <pic@example.com>\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{picture}\r\n--related--\r\n\
         --outer\r\nContent-Type: text/plain; name=\"notes.txt\"\r\n\
         Content-Disposition: attachment; filename=\"notes.txt\"\r\n\r\nSome notes.\r\n--outer--\r\n"
    )
    .into_bytes()
}

fn plain_message(subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Bob <bob@example.com>\r\nTo: Alex <alex@example.com>\r\nSubject: {subject}\r\n\
         Message-ID: <2@example.com>\r\nDate: Tue, 3 Nov 2026 09:00:00 +0000\r\n\r\n{body}\r\n"
    )
    .into_bytes()
}

impl Mail {
    /// muttrs `width` × `height`, as the runtime starts it, its Inbox open.
    fn new(width: u16, height: u16) -> Self {
        Self::with(Config::default(), false, width, height)
    }

    fn with(config: Config, voight: bool, width: u16, height: u16) -> Self {
        let (client, asked) = Client::detached();
        let account = AccountState {
            name: "home".to_owned(),
            client,
            connection: Connection::Online,
            mailboxes: mailboxes(),
            labels: vec![
                muttrs_mail::Label {
                    name: "Work".to_owned(),
                    mailbox: "Labels/Work".to_owned(),
                },
                muttrs_mail::Label {
                    name: "Fun".to_owned(),
                    mailbox: "Labels/Fun".to_owned(),
                },
            ],
            counts: std::collections::HashMap::new(),
            sender: muttrs_mail::compose::Sender {
                addresses: vec![address("Alex")],
                signature: None,
            },
            cache: muttrs_mail::cache::Cache::new(None),
            tasks: crate::tasks::Tasks::default(),
        };
        let (muttrs, kit) = Muttrs::mail(
            config,
            vec![account],
            AddressBook::default(),
            voight,
            crate::pager::Pictures::default(),
        );
        let mut harness = Harness::new(muttrs, kit, width, height);
        let _ = harness.kit.start(&mut harness.app);
        Self { harness, asked }
    }

    /// What was asked of the Account since this was last asked, but for older Envelopes, which an Index near its
    /// end asks for.
    fn requests(&mut self) -> Vec<Request> {
        let mut requests = Vec::new();
        while let Ok((_, request)) = self.asked.try_recv() {
            if !matches!(request, Request::More(_)) {
                requests.push(request);
            }
        }
        requests
    }

    fn press(&mut self, keys: &str) -> Effects {
        self.harness.press(keys)
    }

    /// What the Account tells.
    fn told(&mut self, update: Update) -> Effects {
        self.harness.send(Msg::Account(0, update))
    }

    fn rows(&self) -> Vec<String> {
        self.harness.screen().rows
    }

    fn screen(&self) -> String {
        self.rows().join("\n")
    }

    fn statusline(&self) -> String {
        let rows = self.rows();
        rows[rows.len() - 2].clone()
    }

    fn cmdline(&self) -> String {
        self.rows().last().cloned().unwrap_or_default()
    }

    /// The Inbox, listing `envelopes`.
    fn inbox(&mut self, envelopes: Vec<Envelope>) {
        let exists = u32::try_from(envelopes.len()).unwrap();
        self.told(Update::Envelopes {
            mailbox: "INBOX".to_owned(),
            envelopes,
            exists,
        });
        self.requests();
    }

    /// The Inbox with Ann's Lunch and Bob's Report.
    fn two(&mut self) {
        let mut lunch = envelope(1, 2, "Lunch", "Ann");
        lunch.flags.seen = true;
        self.inbox(vec![lunch, envelope(2, 1, "Report", "Bob")]);
    }

    /// The Message the Account sends for Message `uid` of the Inbox, as asked.
    fn body(&mut self, uid: u32, bytes: Vec<u8>) -> Effects {
        self.told(Update::Body {
            mailbox: "INBOX".to_owned(),
            uid,
            body: Ok(Arc::new(bytes)),
        })
    }

    /// Opens the Message at the cursor, as the Account sends `bytes` for it.
    fn read(&mut self, uid: u32, bytes: Vec<u8>) -> Effects {
        self.press("<CR>");
        assert!(
            self.requests().contains(&Request::Body {
                mailbox: "INBOX".to_owned(),
                uid
            }),
            "the Message is fetched"
        );
        self.body(uid, bytes)
    }
}

fn line_with<'a>(rows: &'a [String], text: &str) -> Option<&'a String> {
    rows.iter().find(|row| row.contains(text))
}

#[test]
fn the_sidebar_lists_the_account_and_its_mailboxes_roles_first_and_folds() {
    let mut mail = Mail::new(100, 20);
    let rows = mail.rows();
    let sidebar: Vec<&str> = rows
        .iter()
        .take(8)
        .map(|row| row.split('│').next().unwrap_or("").trim())
        .collect();
    assert!(sidebar[0].contains("home"), "{sidebar:?}");
    let names: Vec<&str> = sidebar[1..7]
        .iter()
        .map(|row| row.split_whitespace().last().unwrap_or(""))
        .collect();
    assert_eq!(
        names,
        ["Inbox", "Drafts", "Archive", "Trash", "Work", "Reports"]
    );

    // To the sidebar, and its Account's fold closed: one row, saying how many it holds.
    mail.press("<C-w>hggzc");
    let rows = mail.rows();
    assert!(
        rows[0].contains("home") && rows[0].contains("+6"),
        "{rows:#?}"
    );
    assert!(line_with(&rows[1..], "Drafts").is_none(), "{rows:#?}");
    mail.press("zo");
    assert!(line_with(&mail.rows(), "Drafts").is_some());
}

#[test]
fn backslash_goes_to_the_sidebar_hides_it_from_within_and_brings_it_back() {
    let mut mail = Mail::new(100, 20);
    let sidebar = mail.harness.app.sidebar_window();
    assert!(sidebar.is_some());
    assert_ne!(Some(mail.harness.kit.current_window()), sidebar);

    mail.press("\\");
    assert_eq!(Some(mail.harness.kit.current_window()), sidebar, "gone to");
    mail.press("\\");
    assert_eq!(mail.harness.app.sidebar_window(), None, "hidden");
    assert!(!mail.screen().contains('│'), "{}", mail.screen());
    mail.press("\\");
    let back = mail.harness.app.sidebar_window();
    assert!(back.is_some(), "brought back");
    assert_eq!(Some(mail.harness.kit.current_window()), back, "and gone to");
}

#[test]
fn opening_a_mailbox_from_the_sidebar_lists_its_index_in_the_main_window() {
    let mut mail = Mail::new(100, 20);
    assert!(mail.requests().contains(&Request::Open("INBOX".to_owned())));
    mail.press("<C-w>h");
    // The Account, Inbox, Drafts, Archive.
    mail.press("gg3j<CR>");
    assert_eq!(mail.requests(), [Request::Open("Archive".to_owned())]);
    let current = mail.harness.kit.current_buffer();
    assert_eq!(mail.harness.kit.buffer(current).name(), "Archive");
    assert!(mail.harness.kit.buffer(current).is_listed());
    assert!(
        mail.screen().contains("home › Archive"),
        "its winbar: {}",
        mail.screen()
    );
    assert_ne!(
        Some(mail.harness.kit.current_window()),
        mail.harness.app.sidebar_window(),
        "shown beside the sidebar, not in it"
    );
}

#[test]
fn the_index_shows_its_columns_and_its_winbar_counts() {
    let mut mail = Mail::new(100, 20);
    let mut report = envelope(2, 1, "Report", "Bob");
    report.flags.flagged = true;
    report.has_attachments = true;
    let mut lunch = envelope(1, 2, "Lunch", "Ann");
    lunch.flags.seen = true;
    mail.inbox(vec![lunch, report]);
    let rows = mail.rows();
    assert!(
        rows[0].contains("home › Inbox · 2 Messages, 1 Unread"),
        "{rows:#?}"
    );
    let lunch = line_with(&rows, "Lunch").expect("Lunch is listed");
    let report = line_with(&rows, "Report").expect("Report is listed");
    assert!(
        rows.iter().position(|row| row == lunch) < rows.iter().position(|row| row == report),
        "newest first"
    );
    // The flags, the date, whom from and the subject, each in its column whatever the row.
    let column = |row: &str, text: &str| {
        let index = row.split('│').nth(1).expect("beside the sidebar");
        index.find(text).map(|at| index[..at].width())
    };
    for (row, who, subject) in [(lunch, "Ann", "Lunch"), (report, "Bob", "Report")] {
        assert_eq!(column(row, " Nov"), column(lunch, " Nov"), "{row:?}");
        assert_eq!(column(row, who), column(lunch, "Ann"), "{row:?}");
        assert_eq!(column(row, subject), column(lunch, "Lunch"), "{row:?}");
    }
    let report = report.split('│').nth(1).unwrap();
    assert!(
        report.contains('●') || report.contains('\u{f444}'),
        "Unread: {report:?}"
    );
}

#[test]
fn threads_fold_with_the_kits_folds_and_what_is_done_to_a_folded_thread_is_done_to_it_all() {
    let mut mail = Mail::new(100, 20);
    mail.inbox(vec![
        envelope(1, 1, "Lunch", "Ann"),
        reply(2, 2, &[1]),
        reply(3, 4, &[1, 2]),
        envelope(4, 3, "Report", "Cy"),
    ]);
    let rows = mail.rows();
    assert!(line_with(&rows, "└─ Re: Lunch").is_some(), "{rows:#?}");
    mail.press("ggzc");
    let rows = mail.rows();
    assert!(line_with(&rows, "(3) Lunch").is_some(), "{rows:#?}");
    assert!(line_with(&rows, "Re: Lunch").is_none(), "{rows:#?}");
    mail.press("s");
    assert_eq!(
        mail.requests(),
        [Request::Flag {
            mailbox: "INBOX".to_owned(),
            uids: vec![1, 2, 3],
            flag: muttrs_mail::client::Flag::Flagged,
            on: true
        }]
    );
    mail.press("zo]]");
    mail.press("s");
    let flagged: Vec<Vec<u32>> = mail
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            Request::Flag { uids, .. } => Some(uids),
            _ => None,
        })
        .collect();
    assert_eq!(flagged, [vec![4]], "the next thread, the Report alone");
}

#[test]
fn an_html_message_opens_in_the_pager_with_its_picture_asked_for() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    let effects = mail.read(1, html_message("Lunch"));
    let rows = mail.rows();
    assert!(
        rows[0].contains("Lunch"),
        "the winbar says the subject: {rows:#?}"
    );
    assert!(
        line_with(&rows, "From     Ann <ann@example.com>").is_some(),
        "{rows:#?}"
    );
    assert!(
        line_with(&rows, "notes.txt").is_some(),
        "its Attachment: {rows:#?}"
    );
    assert!(line_with(&rows, "Hello the menu[1]").is_some(), "{rows:#?}");
    assert!(mail.statusline().contains("HTML"), "{}", mail.statusline());
    let pictures: Vec<&String> = effects
        .iter()
        .filter_map(|effect| match effect {
            wcgw_kit::Effect::Picture { source, .. } => Some(source),
            _ => None,
        })
        .collect();
    assert_eq!(pictures, ["muttrs:1/0"], "{effects:?}");
    // The kit gets its bytes from what the Pager keeps.
    assert!(
        mail.harness
            .app
            .pictures
            .lock()
            .unwrap()
            .contains_key("muttrs:1/0")
    );

    mail.press("<Space>b");
    assert!(mail.statusline().contains("Plain"), "{}", mail.statusline());
    assert!(line_with(&mail.rows(), "Hello there, in plain text.").is_some());
    mail.press(":raw<CR>");
    assert!(mail.statusline().contains("Source"));
    assert!(line_with(&mail.rows(), "Content-Type: multipart/mixed").is_some());
}

#[test]
fn a_plain_message_shows_with_its_headers_and_q_goes_back_to_the_index() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("j");
    mail.read(
        2,
        plain_message("Report", "The numbers are in.\r\n> Are they?\r\nYes."),
    );
    let rows = mail.rows();
    assert!(
        line_with(&rows, "The numbers are in.").is_some(),
        "{rows:#?}"
    );
    assert!(mail.statusline().contains("Plain"));
    mail.press("K");
    assert!(
        line_with(&mail.rows(), "Message-ID: <2@example.com>").is_some(),
        "every header: {:#?}",
        mail.rows()
    );
    mail.press("q");
    let rows = mail.rows();
    assert!(rows[0].contains("home › Inbox"), "{rows:#?}");
    assert!(mail.harness.kit.cursor_row_text().contains("Report"));
}

#[test]
fn stepping_through_messages_in_the_pager_and_back_to_the_last_one_read() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.read(1, html_message("Lunch"));
    mail.press("<C-n>");
    assert_eq!(
        mail.requests(),
        [Request::Body {
            mailbox: "INBOX".to_owned(),
            uid: 2
        }]
    );
    mail.body(2, plain_message("Report", "The numbers."));
    assert!(mail.rows()[0].contains("Report"));
    mail.press("<C-n>");
    assert!(
        mail.cmdline().contains("No older Message"),
        "{}",
        mail.cmdline()
    );
    mail.press("q");
    assert!(
        mail.harness.kit.cursor_row_text().contains("Report"),
        "the Index's cursor is on the Message read last: {}",
        mail.harness.kit.cursor_row_text()
    );
}

#[test]
fn a_message_shown_a_while_is_marked_read() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("j");
    mail.read(2, plain_message("Report", "The numbers."));
    assert!(mail.requests().is_empty(), "not yet");
    mail.harness.advance(Duration::from_secs(1));
    assert!(mail.requests().contains(&Request::Flag {
        mailbox: "INBOX".to_owned(),
        uids: vec![2],
        flag: muttrs_mail::client::Flag::Seen,
        on: true
    }));
}

/// The text the editor was handed.
fn edited(effects: &Effects) -> Option<String> {
    effects.iter().find_map(|effect| match effect {
        wcgw_kit::Effect::Edit(edit) => Some(edit.text.clone()),
        _ => None,
    })
}

#[test]
fn replying_opens_the_editor_then_the_draft_shows_until_it_is_sent() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.read(1, html_message("Lunch"));
    let text = edited(&mail.press("r")).expect("the editor opens");
    assert!(text.contains("Subject: Re: Lunch"), "{text}");
    assert!(text.contains("To: Ann <ann@example.com>"), "{text}");
    let text = text.replace("\n\n", "\n\nThursday works.\n\n");
    mail.harness.update(wcgw_kit::Msg::Edited(Ok(text)));
    let rows = mail.rows();
    assert!(rows[0].contains("Draft"), "{rows:#?}");
    assert!(line_with(&rows, "Thursday works.").is_some(), "{rows:#?}");
    // <CR> on it edits it again.
    assert!(edited(&mail.press("gg<CR>")).is_some());
    mail.press("<Space>s");
    let sent = mail.requests().into_iter().any(|request| {
        matches!(
            request,
            Request::Send {
                answers: Some(_),
                ..
            }
        )
    });
    assert!(sent, "sent, answering Lunch");
    assert!(
        !mail.rows()[0].contains("Draft"),
        "put away: {:#?}",
        mail.rows()
    );
}

#[test]
fn a_new_draft_left_as_it_was_is_dropped() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("<Space>f");
    assert!(mail.requests().contains(&Request::Body {
        mailbox: "INBOX".to_owned(),
        uid: 1
    }));
    let text = edited(&mail.body(1, plain_message("Lunch", "Thursday?")))
        .expect("forwarding opens the editor, once it's fetched");
    assert!(text.contains("Subject: Fwd: Lunch"), "{text}");
    mail.harness.update(wcgw_kit::Msg::Edited(Ok(text)));
    assert!(mail.cmdline().contains("Not sent"), "{}", mail.cmdline());
}

#[test]
fn a_new_message_asks_whom_to_then_its_subject_then_opens_the_editor() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("c");
    assert!(mail.cmdline().starts_with("To:"), "{}", mail.cmdline());
    mail.press("ann@example.com<CR>");
    assert!(mail.cmdline().starts_with("Subject:"), "{}", mail.cmdline());
    let text = edited(&mail.press("Lunch?<CR>")).expect("the editor");
    assert!(text.contains("To: ann@example.com"), "{text}");
    assert!(text.contains("Subject: Lunch?"), "{text}");
}

#[test]
fn compose_completes_addresses_seen() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    let completed = mail
        .harness
        .kit
        .ex_completions(&mail.harness.app, "compose an");
    assert_eq!(completed.candidates, ["ann@example.com"]);
}

#[test]
fn filters_select_and_narrow_and_complete() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    let completed = mail
        .harness
        .kit
        .ex_completions(&mail.harness.app, "select is:un");
    assert_eq!(completed.candidates, ["is:unread"]);
    let completed = mail
        .harness
        .kit
        .ex_completions(&mail.harness.app, "filter from:b");
    assert_eq!(completed.candidates, ["from:bob@example.com"]);

    mail.press(":select from:bob<CR>");
    assert!(
        mail.cmdline().contains("1 Message selected"),
        "{}",
        mail.cmdline()
    );
    assert!(mail.rows()[0].contains("1 selected"));
    assert!(
        line_with(&mail.rows(), "▌").is_some(),
        "the Selection's mark"
    );
    // What's done is done to the Selection, wherever the cursor is.
    mail.press("gga");
    assert_eq!(
        mail.requests(),
        [Request::Archive {
            mailbox: "INBOX".to_owned(),
            uids: vec![2]
        }]
    );
    mail.press(":filter from:ann<CR>");
    assert!(
        mail.rows()[0].contains("filter from:a"),
        "{:#?}",
        mail.rows()
    );
    mail.press(":filter<CR>");
    assert!(mail.cmdline().contains("All show again"));
}

#[test]
fn ctrl_l_asks_for_what_s_new_and_sync_lists_the_mailbox_anew() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    let _ = mail.requests();
    mail.press("<C-l>");
    assert_eq!(mail.requests(), [Request::Refresh]);
    mail.press(":sync<CR>");
    assert_eq!(mail.requests(), [Request::Resync("INBOX".to_owned())]);
}

#[test]
fn x_selects_and_goes_down_and_esc_clears_the_selection_then_the_filter() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    // The second `x` selects the next Message, not the first again.
    mail.press("xx");
    assert!(mail.rows()[0].contains("2 selected"), "{:#?}", mail.rows());
    mail.press(":unselect<CR>");
    assert!(!mail.rows()[0].contains("selected"));
    // A selects all listed, or none once they all are, as ghd's does.
    mail.press("ggxA");
    assert!(mail.rows()[0].contains("2 selected"), "{:#?}", mail.rows());
    mail.press("A");
    assert!(!mail.rows()[0].contains("selected"), "{:#?}", mail.rows());

    // Lunch, from Ann, selected, then left out of what's listed.
    mail.press("ggx:filter from:bob<CR>");
    assert!(mail.rows()[0].contains("1 selected"), "{:#?}", mail.rows());
    assert!(mail.rows()[0].contains("filter from:bob"));
    // `<Esc>` unselects what isn't listed too, and leaves the filter for the next one.
    mail.press("<Esc>");
    assert!(!mail.rows()[0].contains("selected"), "{:#?}", mail.rows());
    assert!(mail.rows()[0].contains("filter from:bob"));
    mail.press("<Esc>");
    assert!(!mail.rows()[0].contains("filter"), "{:#?}", mail.rows());
    assert!(mail.cmdline().contains("All show again"));
}

#[test]
fn links_open_from_the_cursor_or_a_picker_and_copy_from_another() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.read(1, html_message("Lunch"));
    mail.press("<Space>o");
    let screen = mail.screen();
    assert!(screen.contains("https://example.com/menu"), "{screen}");
    let effects = mail.press("<CR>");
    assert!(effects.contains(&wcgw_kit::Effect::OpenUrl(
        "https://example.com/menu".to_owned()
    )));
    let effects = mail.press("<Space>o<C-y>");
    assert!(
        effects.contains(&wcgw_kit::Effect::Copy(
            "https://example.com/menu".to_owned()
        )),
        "{effects:?}"
    );
    assert!(
        mail.screen().contains("Links"),
        "copying leaves the picker open"
    );
    // On the Link itself, <CR> opens it, as gx does, and gy yanks it.
    mail.press("<Esc>/menu<CR>");
    for keys in ["<CR>", "gx"] {
        let effects = mail.press(keys);
        assert!(
            effects.contains(&wcgw_kit::Effect::OpenUrl(
                "https://example.com/menu".to_owned()
            )),
            "{keys}: {effects:?}"
        );
    }
    let effects = mail.press("gy");
    assert!(
        effects.contains(&wcgw_kit::Effect::Copy(
            "https://example.com/menu".to_owned()
        )),
        "{effects:?}"
    );
    // :browse alone is the Message's own page, as in the other apps, which mail hasn't.
    let effects = mail.press(":browse<CR>");
    assert!(effects.is_empty(), "{effects:?}");
    assert!(
        mail.cmdline().contains(":browse link opens a Link"),
        "{}",
        mail.cmdline()
    );
}

#[test]
fn attachments_open_or_save_from_pickers() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.read(1, html_message("Lunch"));
    mail.press("<Space>a");
    assert!(mail.screen().contains("notes.txt"), "{}", mail.screen());
    let effects = mail.press("<CR>");
    assert!(effects.iter().any(|effect| matches!(
        effect,
        wcgw_kit::Effect::App(Io::OpenFile { name, .. }) if name == "notes.txt"
    )));
    let effects = mail.press("<Space>a<C-s>");
    assert!(effects.iter().any(|effect| matches!(
        effect,
        wcgw_kit::Effect::App(Io::Save { name, .. }) if name == "notes.txt"
    )));
    assert!(
        !mail.screen().contains("Attachments"),
        "saved, the list goes: {}",
        mail.screen()
    );
}

#[test]
fn moving_and_labelling_choose_from_pickers() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("<Space>mArchive<CR>");
    assert_eq!(
        mail.requests(),
        [Request::Move {
            mailbox: "INBOX".to_owned(),
            uids: vec![1],
            to: "Archive".to_owned()
        }]
    );
    assert!(
        line_with(&mail.rows(), "Lunch").is_none(),
        "gone from the Index"
    );
    mail.press("<Space>lFun<CR>");
    assert!(
        mail.requests().iter().any(|request| matches!(
            request,
            Request::Label { uids, on: true, .. } if uids == &[2]
        )),
        "the Report labelled"
    );
    assert!(
        mail.screen().contains("✓ Fun"),
        "the picker, still open: {}",
        mail.screen()
    );
    mail.press("<CR>");
    assert!(
        mail.requests().iter().any(|request| matches!(
            request,
            Request::Label { uids, on: false, .. } if uids == &[2]
        )),
        "taken off again"
    );
    mail.press("<Esc>");
    assert!(!mail.screen().contains("Fun"), "{}", mail.screen());
}

#[test]
fn keys_follow_the_shared_verbs() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("dd");
    assert_eq!(
        mail.requests(),
        [Request::Delete {
            mailbox: "INBOX".to_owned(),
            uids: vec![1]
        }]
    );
    assert!(
        mail.harness.kit.cursor_row_text().contains("Report"),
        "on what followed"
    );
    mail.press("u");
    assert!(matches!(
        mail.requests().as_slice(),
        [Request::MoveBack { message_ids, .. }] if message_ids == &["1@example.com"]
    ));
    mail.press("U");
    assert!(matches!(
        mail.requests().as_slice(),
        [Request::Flag {
            flag: muttrs_mail::client::Flag::Seen,
            on: true,
            ..
        }]
    ));
    mail.press("<C-l>");
    assert_eq!(mail.requests(), [Request::Refresh]);
    mail.press("?Rep<CR>");
    assert!(
        mail.harness.kit.cursor_row_text().contains("Report"),
        "? searches back"
    );
}

#[test]
fn searching_the_server_asks_at_a_prompt_and_lists_what_it_found() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("S");
    assert!(mail.cmdline().starts_with("Search the server:"));
    mail.press("lunch<CR>");
    assert_eq!(
        mail.requests(),
        [Request::Search {
            mailbox: "INBOX".to_owned(),
            query: "lunch".to_owned()
        }]
    );
    mail.told(Update::Found {
        mailbox: "INBOX".to_owned(),
        query: "lunch".to_owned(),
        envelopes: vec![envelope(1, 2, "Lunch", "Ann")],
        count: 1,
    });
    assert!(
        mail.rows()[0].contains("“lunch” · 1 found"),
        "{:#?}",
        mail.rows()
    );
    mail.press("q");
    assert!(
        mail.rows()[0].contains("2 Messages"),
        "back: {:#?}",
        mail.rows()
    );
}

#[test]
fn a_search_of_every_mailbox_lists_where_it_found_messages() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press(":search report<CR>");
    assert_eq!(
        mail.requests(),
        [Request::SearchAll {
            query: "report".to_owned()
        }]
    );
    mail.told(Update::FoundIn {
        query: "report".to_owned(),
        found: vec![("Work".to_owned(), 3), ("INBOX".to_owned(), 1)],
    });
    assert!(
        mail.screen().contains("4 found: report"),
        "{}",
        mail.screen()
    );
    mail.press("Work<CR>");
    let requests = mail.requests();
    assert!(
        requests.contains(&Request::Open("Work".to_owned())),
        "{requests:?}"
    );
    assert!(requests.contains(&Request::Search {
        mailbox: "Work".to_owned(),
        query: "report".to_owned()
    }));
}

#[test]
fn piping_fetches_the_messages_then_runs_the_command() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("|");
    assert!(mail.cmdline().starts_with('|'));
    mail.press("cat<CR>");
    assert_eq!(
        mail.requests(),
        [Request::Body {
            mailbox: "INBOX".to_owned(),
            uid: 1
        }]
    );
    let effects = mail.body(1, plain_message("Lunch", "Hi."));
    let shell = effects
        .iter()
        .find_map(|effect| match effect {
            wcgw_kit::Effect::Shell(shell) => Some(shell),
            _ => None,
        })
        .expect("the terminal handed to the command");
    assert_eq!(shell.command, "cat");
    assert!(shell.wait, "Enter brings muttrs back");
    let piped = String::from_utf8_lossy(shell.stdin.as_deref().unwrap_or_default());
    assert!(
        piped.contains("Subject: Lunch") && piped.contains("Hi."),
        "the Message, as it came: {piped}"
    );
}

#[test]
fn bang_runs_a_command_with_the_terminal_and_alone_the_shell() {
    let mut mail = Mail::new(100, 30);
    let effects = mail.press("!date<CR>");
    assert!(effects.contains(&wcgw_kit::Effect::Shell(
        wcgw_kit::Shell::new("date").wait()
    )));
    let effects = mail.press("!<CR>");
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            wcgw_kit::Effect::Shell(shell) if !shell.wait && shell.stdin.is_none()
        )),
        "{effects:?}"
    );
}

#[test]
fn drafts_are_edited_as_mail_in_the_editor_the_configuration_says() {
    let config = Config {
        editor: Some("nvim -c startinsert".to_owned()),
        ..Config::default()
    };
    let mut mail = Mail::with(config, false, 100, 30);
    mail.two();
    let effects = mail.press("c<CR>");
    assert!(effects.is_empty(), "nobody: no Draft");
    let effects = mail.press("cann@example.com<CR>Lunch<CR>");
    let edit = effects
        .iter()
        .find_map(|effect| match effect {
            wcgw_kit::Effect::Edit(edit) => Some(edit),
            _ => None,
        })
        .expect("the editor opens");
    assert_eq!(edit.suffix, "eml");
    assert_eq!(edit.editor.as_deref(), Some("nvim -c startinsert"));
    let mut mail = Mail::new(100, 30);
    mail.two();
    let effects = mail.press("cann@example.com<CR>Lunch<CR>");
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            wcgw_kit::Effect::Edit(edit) if edit.editor.is_none()
        )),
        "the user's editor, without one configured"
    );
}

#[test]
fn a_double_click_opens_the_message_clicked() {
    let mut mail = Mail::new(100, 20);
    mail.two();
    let rows = mail.rows();
    let row = rows.iter().position(|row| row.contains("Report")).unwrap();
    let column = u16::try_from(rows[row].find("Report").unwrap()).unwrap();
    let row = u16::try_from(row).unwrap();
    mail.harness.update(wcgw_kit::Msg::Mouse {
        mouse: wcgw_kit::key::Mouse::DoubleLeft,
        column,
        row,
    });
    assert_eq!(
        mail.requests(),
        [Request::Body {
            mailbox: "INBOX".to_owned(),
            uid: 2
        }]
    );
}

#[test]
fn the_statusline_spins_while_tasks_wait_and_quitting_waits_for_them() {
    let mut mail = Mail::new(100, 20);
    mail.two();
    mail.press("<C-l>");
    // The spinner turns while Tasks wait.
    let effects = mail.harness.advance(super::SPIN);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, wcgw_kit::Effect::After { msg: Msg::Tick, .. }))
    );
    assert!(
        mail.statusline().contains("Opening Inbox  1 more"),
        "{}",
        mail.statusline()
    );
    let effects = mail.press("q");
    assert!(!effects.contains(&wcgw_kit::Effect::Quit));
    assert!(
        mail.cmdline().contains("Quitting once"),
        "{}",
        mail.cmdline()
    );
    // The Tasks finished: quitting, as asked.
    let mut quit = false;
    for task in mail.harness.app.accounts[0].tasks.waiting() {
        quit |= mail
            .told(Update::Finished(task))
            .contains(&wcgw_kit::Effect::Quit);
    }
    assert!(quit);
}

#[test]
fn icons_come_from_the_kit_and_set_icons_plain_draws_them_plain() {
    let mut mail = Mail::new(100, 20);
    mail.two();
    mail.press(":set icons=plain<CR>");
    // Anything that lays the Index out again, as a key does: icons are drawn into the text.
    mail.press("j");
    assert!(line_with(&mail.rows(), "●").is_some(), "{:#?}", mail.rows());
}

#[test]
fn the_configuration_s_look_is_read_no_more_and_says_so_as_ghd_s() {
    let config = Config {
        theme: Some(toml::Value::String("terminal".to_owned())),
        ..Config::default()
    };
    let mail = Mail::with(config, false, 100, 20);
    assert_eq!(mail.harness.kit.theme().name(), "tokyonight-moon");
    assert!(
        mail.cmdline().contains("theme read no more"),
        "{}",
        mail.cmdline()
    );
}

#[cfg(feature = "voight")]
#[test]
fn voight_assesses_the_pager_s_prose_says_so_and_shades_its_heatmap() {
    use crate::assessment::{Assessment, Event, Window};
    let mut mail = Mail::with(Config::default(), true, 100, 30);
    mail.two();
    mail.harness
        .send(Msg::Voight(Event::Loaded { fallback: None }));
    let prose = format!("{}see https://example.com/x for more.", "words ".repeat(60));
    mail.press("j");
    let effects = mail.read(2, plain_message("Report", &prose));
    let key = effects
        .iter()
        .find_map(|effect| match effect {
            wcgw_kit::Effect::App(Io::Assess { key, .. }) => Some(key.clone()),
            _ => None,
        })
        .expect("assessed");
    assert!(
        mail.statusline().contains("voight …"),
        "{}",
        mail.statusline()
    );
    mail.harness.send(Msg::Voight(Event::Assessed {
        key: key.clone(),
        assessment: Ok(Some(Assessment {
            share: 0.73,
            windows: Arc::from([Window {
                range: 0..key.len(),
                share: 0.73,
            }]),
        })),
    }));
    assert!(
        mail.statusline().contains("voight 73%"),
        "{}",
        mail.statusline()
    );
    mail.press("<Space>h");
    assert!(
        mail.statusline().contains("Heatmap"),
        "{}",
        mail.statusline()
    );
    let rows = mail.rows();
    let y = rows.iter().position(|row| row.contains("words")).unwrap();
    let x = u16::try_from(rows[y].find("words").unwrap()).unwrap();
    let shaded = mail.harness.style_at(x, u16::try_from(y).unwrap());
    assert!(shaded.bg.is_some(), "{shaded:?}");
    // A Link in shaded prose still opens.
    mail.press("/x for<CR>");
    let effects = mail.press("<CR>");
    assert!(
        effects.contains(&wcgw_kit::Effect::OpenUrl(
            "https://example.com/x".to_owned()
        )),
        "{effects:?}"
    );
    mail.press(":voight off<CR>");
    assert!(!mail.statusline().contains("Heatmap"));
}

#[test]
fn view_shows_a_message_alone_and_q_quits() {
    let message = muttrs_mail::Message::parse(plain_message("Report", "Numbers.")).unwrap();
    let (muttrs, kit) = Muttrs::view(
        Config::default(),
        message,
        false,
        crate::pager::Pictures::default(),
    );
    let mut harness = Harness::new(muttrs, kit, 80, 20);
    let _ = harness.kit.start(&mut harness.app);
    let rows = harness.screen().rows;
    assert!(line_with(&rows, "Numbers.").is_some(), "{rows:#?}");
    assert!(harness.press("q").contains(&wcgw_kit::Effect::Quit));
}

#[test]
fn help_lists_muttrs_commands_and_keys() {
    let mut mail = Mail::new(100, 40);
    mail.press(":help<CR>/reply to all<CR>");
    assert!(
        mail.harness.kit.cursor_row_text().contains("reply all"),
        "{}",
        mail.harness.kit.cursor_row_text()
    );
    mail.press("gg/delete the Messages<CR>");
    assert!(mail.harness.kit.cursor_row_text().contains("delete"));
}

#[test]
fn the_mini_index_keeps_some_of_the_index_above_the_pager_on_the_message_read() {
    let config = Config {
        pager_index_lines: 4,
        ..Config::default()
    };
    let mut mail = Mail::with(config, false, 100, 30);
    mail.two();
    mail.read(1, html_message("Lunch"));
    assert_eq!(
        mail.harness.kit.window_ids().len(),
        3,
        "the sidebar, the mini-index, the Pager"
    );
    let rows = mail.rows();
    let lunch = rows
        .iter()
        .position(|row| row.contains("Ann") && row.contains("Lunch"));
    let from = rows.iter().position(|row| row.contains("From     Ann"));
    assert!(lunch < from, "the Index above the Pager: {rows:#?}");
    mail.press("<C-n>");
    mail.body(2, plain_message("Report", "The numbers."));
    assert_eq!(mail.harness.kit.window_ids().len(), 3);
    mail.press("q");
    assert_eq!(
        mail.harness.kit.window_ids().len(),
        2,
        "back to the sidebar and the Index"
    );
}

#[test]
fn the_cursor_s_row_is_drawn_across_the_index() {
    let mut mail = Mail::new(100, 20);
    mail.two();
    mail.press("j");
    let rows = mail.rows();
    let y = rows.iter().position(|row| row.contains("Report")).unwrap();
    let cursor = mail.harness.kit.style(wcgw_kit::hl::Group::CURSOR_LINE).bg;
    assert!(cursor.is_some());
    assert_eq!(
        mail.harness.style_at(99, u16::try_from(y).unwrap()).bg,
        cursor,
        "to the edge"
    );
    let lunch = rows.iter().position(|row| row.contains("Lunch")).unwrap();
    assert_ne!(
        mail.harness.style_at(99, u16::try_from(lunch).unwrap()).bg,
        cursor
    );
}

#[test]
fn an_account_in_the_sidebar_asks_what_s_new_and_cd_opens_a_mailbox() {
    let mut mail = Mail::new(100, 20);
    mail.requests();
    mail.press("<C-w>hgg<CR>");
    assert_eq!(mail.requests(), [Request::Refresh]);
    mail.press(":e work/reports<CR>");
    assert_eq!(mail.requests(), [Request::Open("Work/Reports".to_owned())]);
    let current = mail.harness.kit.current_buffer();
    assert_eq!(mail.harness.kit.buffer(current).name(), "Reports");
    mail.press(":e Archive<CR>");
    assert_eq!(mail.requests(), [Request::Open("Archive".to_owned())]);
}

#[test]
fn q_closes_help_s_window_and_goes_back_from_signing_in() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press(":help<CR>");
    assert_eq!(mail.harness.kit.window_ids().len(), 3);
    let effects = mail.press("q");
    assert!(!effects.contains(&wcgw_kit::Effect::Quit));
    assert_eq!(mail.harness.kit.window_ids().len(), 2);
    // A buffer of muttrs' own, as signing in shows, is left for the Index.
    let sign_in = mail
        .harness
        .kit
        .add_buffer("login", super::Document::new(), false);
    mail.harness.kit.show_buffer(sign_in);
    mail.press("q");
    assert!(
        mail.rows()[0].contains("home › Inbox"),
        "{:#?}",
        mail.rows()
    );
}

#[test]
fn the_to_prompt_completes_addresses_seen() {
    let mut mail = Mail::new(100, 30);
    mail.two();
    mail.press("cbob@example.com, an<Tab>");
    assert!(
        mail.cmdline()
            .contains("To: bob@example.com, ann@example.com"),
        "{}",
        mail.cmdline()
    );
}

#[test]
fn quitting_with_tasks_left_waits_for_them_but_not_with_a_bang() {
    let mut mail = Mail::new(100, 20);
    mail.two();
    assert!(!mail.press(":qa<CR>").contains(&wcgw_kit::Effect::Quit));
    assert!(
        mail.cmdline().contains("Quitting once"),
        "{}",
        mail.cmdline()
    );
    assert!(mail.press(":qa!<CR>").contains(&wcgw_kit::Effect::Quit));
}

#[test]
fn the_sidebar_s_cursor_follows_the_mailbox_opened_by_name() {
    let mut mail = Mail::new(100, 20);
    mail.press(":e Archive<CR><C-w>h");
    assert!(
        mail.harness.kit.cursor_row_text().contains("Archive"),
        "{}",
        mail.harness.kit.cursor_row_text()
    );
}

#[test]
fn the_index_is_laid_out_for_its_window_s_width() {
    let mut mail = Mail::new(120, 20);
    mail.two();
    mail.press(":vsplit<CR>");
    let width = mail
        .harness
        .kit
        .text_width(mail.harness.kit.current_buffer());
    assert!(width < 60, "{width}");
    let rows = mail.rows();
    let lunch = line_with(&rows, "Lunch").expect("listed");
    assert_eq!(
        lunch.matches("Lunch").count(),
        2,
        "on a row in each window, not wrapped: {rows:#?}"
    );
    assert_eq!(
        line_with(&rows, "Report")
            .unwrap()
            .matches("Report")
            .count(),
        2
    );
}
