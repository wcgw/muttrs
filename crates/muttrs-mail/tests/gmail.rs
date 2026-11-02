//! Gmail's Threads (`X-GM-THRID`), against a server on the loopback that answers as Gmail does: neither GreenMail
//! nor Dovecot has Gmail's extensions.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use muttrs_mail::account::{Endpoint, Tls};
use muttrs_mail::imap::{Credentials, Session};
use muttrs_mail::thread;
use muttrs_mail::tls::{self, Trust};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;

/// The header fields of the Inbox's Messages: b replies to a without saying so (as some clients send it), c is
/// apart; Gmail has a and b in one Thread.
const HEADERS: [&str; 3] = [
    "Subject: a\r\nMessage-ID: <a@example.com>\r\nDate: Mon, 2 Nov 2026 10:00:00 +0000\r\n\r\n",
    "Subject: Re: a\r\nMessage-ID: <b@example.com>\r\nDate: Mon, 2 Nov 2026 11:00:00 +0000\r\n\r\n",
    "Subject: c\r\nMessage-ID: <c@example.com>\r\nDate: Mon, 2 Nov 2026 12:00:00 +0000\r\n\r\n",
];

/// A Gmail-like server on the loopback, which tells the Threads if `tells`, or refuses to; and how many times it
/// was asked.
async fn gmail(tells: bool) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let (read, mut write) = tcp.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"* OK Gimap ready\r\n").await.unwrap();
        while let Some(line) = lines.next_line().await.unwrap() {
            let (tag, command) = line.split_once(' ').unwrap();
            let command = command.to_ascii_uppercase();
            if command.contains("X-GM-THRID") {
                counted.fetch_add(1, Ordering::SeqCst);
            }
            let (untagged, status) = answer(&command, tells);
            write
                .write_all(format!("{untagged}{tag} {status} done\r\n").as_bytes())
                .await
                .unwrap();
        }
    });
    (port, asked)
}

/// What the server answers `command`, before its status.
fn answer(command: &str, tells: bool) -> (String, &'static str) {
    if command.starts_with("CAPABILITY") {
        (
            "* CAPABILITY IMAP4rev1 UIDPLUS MOVE IDLE X-GM-EXT-1\r\n".into(),
            "OK",
        )
    } else if command.starts_with("SELECT") {
        (
            "* 3 EXISTS\r\n* OK [UIDVALIDITY 11] UIDs valid.\r\n* OK [UIDNEXT 4] Predicted next UID.\r\n".into(),
            "OK [READ-WRITE]",
        )
    } else if command.contains("X-GM-THRID") {
        if tells {
            // As Gmail orders them, with a Flags change meanwhile.
            (
                "* 1 FETCH (X-GM-THRID 1001 UID 1)\r\n* 2 FETCH (X-GM-THRID 1001 UID 2)\r\n\
                 * 3 FETCH (FLAGS (\\Seen))\r\n* 3 FETCH (X-GM-THRID 1003 UID 3)\r\n"
                    .into(),
                "OK",
            )
        } else {
            (String::new(), "BAD")
        }
    } else if command.contains("FETCH") {
        let mut fetched = String::new();
        for (headers, n) in HEADERS.iter().zip(1..) {
            let _ = write!(
                fetched,
                "* {n} FETCH (UID {n} FLAGS (\\Seen) INTERNALDATE \"02-Nov-2026 10:00:00 +0000\" RFC822.SIZE 100 \
                 BODY[HEADER.FIELDS (SUBJECT MESSAGE-ID DATE)] {{{}}}\r\n{headers})\r\n",
                headers.len()
            );
        }
        (fetched, "OK")
    } else {
        // LOGIN, LOGOUT.
        (String::new(), "OK")
    }
}

async fn signed_in(port: u16) -> Session {
    let server = Endpoint {
        host: "127.0.0.1".into(),
        port,
        tls: Tls::None,
    };
    let credentials = Credentials::Password {
        username: "ann@gmail.com".into(),
        password: "secret".into(),
    };
    let config = tls::client_config(&Trust::default()).unwrap();
    let mut session = Session::connect(&server, config, &credentials)
        .await
        .unwrap();
    session.select("INBOX").await.unwrap();
    session
}

#[tokio::test]
async fn gmails_threads_join_what_references_dont() {
    let (port, asked) = gmail(true).await;
    let mut session = signed_in(port).await;
    let mut envelopes = session.newest(3, 50).await.unwrap();
    let threads: Vec<_> = envelopes.iter().map(|e| e.gmail_thread).collect();
    assert_eq!(threads, [Some(1001), Some(1001), Some(1003)]);
    let rows: Vec<(u32, usize)> = thread::threads(&envelopes)
        .iter()
        .map(|row| (envelopes[row.envelope].uid, row.depth))
        .collect();
    assert_eq!(rows, [(3, 0), (1, 0), (2, 1)], "b under a, c the latest");

    // Kept without their Threads, as by an earlier muttrs: asked once, then not again.
    for envelope in &mut envelopes {
        envelope.gmail_thread = None;
    }
    session.add_gmail_threads(&mut envelopes).await.unwrap();
    session.add_gmail_threads(&mut envelopes).await.unwrap();
    assert_eq!(envelopes[2].gmail_thread, Some(1003));
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    session.logout().await;
}

#[tokio::test]
async fn a_server_that_wont_tell_threads_is_still_listed() {
    let (port, asked) = gmail(false).await;
    let mut session = signed_in(port).await;
    let envelopes = session.newest(3, 50).await.unwrap();
    assert_eq!(envelopes.len(), 3);
    assert!(envelopes.iter().all(|e| e.gmail_thread.is_none()));
    assert_eq!(envelopes[1].subject, "Re: a", "the connection goes on");
    session.newest(3, 50).await.unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 1, "not asked again");
    session.logout().await;
}
