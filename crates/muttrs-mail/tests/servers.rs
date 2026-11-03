//! IMAP against real servers: GreenMail and Dovecot, in containers that `scripts/test-servers.sh up` starts. Ignored
//! unless asked for:
//!
//! ```sh
//! MUTTRS_TEST_CA=target/test-servers/ca.pem cargo test -p muttrs-mail --test servers -- --ignored
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use muttrs_mail::account::{Endpoint, Tls};
use muttrs_mail::cache::{Cache, Listed};
use muttrs_mail::client::{Client, Connection, Request, Update, Went};
use muttrs_mail::imap::{Credentials, Error, Session};
use muttrs_mail::tls::Trust;
use muttrs_mail::{AccountConfig, Envelope, Flags, Message, Role, tls};

fn ca() -> std::path::PathBuf {
    std::env::var_os("MUTTRS_TEST_CA")
        .expect("MUTTRS_TEST_CA names the test servers' CA: see scripts/test-servers.sh")
        .into()
}

/// Trusting the test servers' CA.
fn roots() -> Trust {
    Trust::Roots {
        ca_file: Some(ca()),
    }
}

/// Trusting the test servers' own certificate as it is, which `scripts/test-servers.sh` keeps next to the CA.
fn pinned() -> Trust {
    Trust::Pinned(ca().with_file_name("server.pem"))
}

/// A user no other test, nor an earlier run, has used.
fn user(test: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{test}-{now}@localhost")
}

fn endpoint(port: u16, tls: Tls) -> Endpoint {
    Endpoint {
        host: "localhost".into(),
        port,
        tls,
    }
}

fn password(username: &str, password: &str) -> Credentials {
    Credentials::Password {
        username: username.into(),
        password: password.into(),
    }
}

fn message(subject: &str) -> Vec<u8> {
    format!(
        "From: Ann <ann@example.com>\r\nTo: you@localhost\r\nSubject: {subject}\r\n\
         Date: Mon, 2 Nov 2026 10:00:00 +0000\r\nMessage-ID: <{subject}@example.com>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nHello from {subject}.\r\n"
    )
    .into_bytes()
}

/// Reads, flags and moves Messages over `session`.
async fn exercise(mut session: Session) {
    let names: Vec<String> = session
        .mailboxes()
        .await
        .unwrap()
        .into_iter()
        .map(|mailbox| mailbox.name)
        .collect();
    assert!(
        names.iter().any(|name| name.eq_ignore_ascii_case("INBOX")),
        "{names:?}"
    );

    session
        .append("INBOX", "()", &message("first"))
        .await
        .unwrap();
    session
        .append("INBOX", "(\\Seen)", &message("second"))
        .await
        .unwrap();
    let selected = session.select("INBOX").await.unwrap();
    assert_eq!(selected.exists, 2);
    assert_eq!(session.unread().await.unwrap(), 1);

    let envelopes = session.newest(selected.exists, 10).await.unwrap();
    let subjects: Vec<&str> = envelopes
        .iter()
        .map(|envelope| envelope.subject.as_str())
        .collect();
    assert_eq!(subjects, ["first", "second"]);
    assert_eq!(envelopes[0].from[0].short(), "Ann");
    assert!(!envelopes[0].flags.seen && envelopes[1].flags.seen);

    let first = envelopes[0].uid;
    let body = Message::parse(session.body(first).await.unwrap()).unwrap();
    assert_eq!(body.plain().unwrap().0.trim(), "Hello from first.");
    // Fetching the body didn't mark it Seen: PEEK.
    assert!(!session.flags(&first.to_string()).await.unwrap()[0].1.seen);

    session.set_flag(&[first], "\\Seen", true).await.unwrap();
    assert!(session.flags(&first.to_string()).await.unwrap()[0].1.seen);

    session.create("Archive").await.unwrap();
    session.move_to(&[first], "Archive").await.unwrap();
    let remaining = session.envelopes("1:*").await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].subject, "second");
    let archived = session.counts("Archive").await.unwrap();
    assert_eq!(archived.messages, 1);
    session.logout().await;
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn greenmail_over_tls() {
    let config = tls::client_config(&roots()).unwrap();
    let username = user("greenmail-tls");
    let session = Session::connect(
        &endpoint(3993, Tls::Ssl),
        config,
        &password(&username, "any"),
    )
    .await
    .unwrap();
    exercise(session).await;
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn greenmail_with_xoauth2() {
    let config = tls::client_config(&roots()).unwrap();
    let credentials = Credentials::Bearer {
        username: user("greenmail-xoauth2"),
        token: "ya29.any-token".into(),
    };
    let mut session = Session::connect(&endpoint(3993, Tls::Ssl), config, &credentials)
        .await
        .unwrap();
    assert_ne!(session.mailboxes().await.unwrap(), []);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn dovecot_with_starttls() {
    let config = tls::client_config(&roots()).unwrap();
    let username = user("dovecot-starttls");
    let session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        config,
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    exercise(session).await;
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn dovecot_over_tls() {
    let config = tls::client_config(&roots()).unwrap();
    let username = user("dovecot-tls");
    let mut session = Session::connect(
        &endpoint(31993, Tls::Ssl),
        config,
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    assert!(session.can_idle());
    assert_ne!(session.mailboxes().await.unwrap(), []);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn no_starttls_no_sign_in() {
    let config = tls::client_config(&roots()).unwrap();
    let error = Session::connect(
        &endpoint(3143, Tls::Starttls),
        config,
        &password("a@localhost", "any"),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::NoStarttls(_)), "{error}");
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn a_wrong_password_is_a_sign_in_failure() {
    let config = tls::client_config(&roots()).unwrap();
    let error = Session::connect(
        &endpoint(31993, Tls::Ssl),
        config,
        &password(&user("wrong"), "wrong"),
    )
    .await
    .unwrap_err();
    assert!(error.is_sign_in(), "{error}");
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn without_the_ca_the_certificate_is_refused() {
    let config = tls::client_config(&Trust::default()).unwrap();
    let error = Session::connect(
        &endpoint(31993, Tls::Ssl),
        config,
        &password(&user("ca"), "secret"),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::Tls(_)), "{error}");
}

/// Waits for the first update `wanted` takes, failing after a while.
async fn until<T>(
    updates: &mut tokio::sync::mpsc::UnboundedReceiver<Update>,
    mut wanted: impl FnMut(&Update) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let update = updates.recv().await.expect("updates");
            if let Update::Connection(Connection::SignInFailed(why) | Connection::Offline(why)) =
                &update
            {
                panic!("{why}");
            }
            if let Some(found) = wanted(&update) {
                return found;
            }
        }
    })
    .await
    .expect("the update in time")
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn an_accounts_client_lists_opens_reads_and_sees_new_mail() {
    let username = user("client");
    // Messages there before the client connects.
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    session
        .append("INBOX", "()", &message("waiting"))
        .await
        .unwrap();
    session.create("Trash").await.unwrap();

    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}\n[mailboxes]\ntrash = \"Trash\"",
        ca().display().to_string()
    ))
    .unwrap();
    let account = config.resolve().unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let cache = std::env::temp_dir().join(format!("muttrs-client-test-{}", std::process::id()));
    let client = Client::spawn(
        account,
        Cache::new(Some(cache.clone())),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();

    let mailboxes = until(&mut updates, |update| match update {
        Update::Mailboxes(mailboxes) => Some(mailboxes.clone()),
        _ => None,
    })
    .await;
    assert!(
        mailboxes
            .iter()
            .any(|mailbox| mailbox.role == Some(Role::Inbox))
    );

    client.send(Request::Open("INBOX".into()));
    let envelopes = until(&mut updates, |update| match update {
        Update::Envelopes {
            mailbox, envelopes, ..
        } if mailbox == "INBOX" => Some(envelopes.clone()),
        _ => None,
    })
    .await;
    assert_eq!(envelopes.len(), 1);
    let uid = envelopes[0].uid;

    client.send(Request::Body {
        mailbox: "INBOX".into(),
        uid,
    });
    let body = until(&mut updates, |update| match update {
        Update::Body { body, .. } => Some(body.clone()),
        _ => None,
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("Hello from waiting."));

    // New mail, by IDLE, from another session.
    session
        .append("INBOX", "()", &message("arriving"))
        .await
        .unwrap();
    let envelopes = until(&mut updates, |update| match update {
        Update::Envelopes { envelopes, .. } if envelopes.len() == 2 => Some(envelopes.clone()),
        _ => None,
    })
    .await;
    assert_eq!(envelopes[1].subject, "arriving");

    client.send(Request::Delete {
        mailbox: "INBOX".into(),
        uids: vec![uid],
    });
    let done = until(&mut updates, |update| match update {
        Update::Done(done) => Some(done.clone()),
        Update::Failed(failed) => panic!("{failed}"),
        _ => None,
    })
    .await;
    assert_eq!(done, "Moved a Message to Trash");
    assert_eq!(session.counts("Trash").await.unwrap().messages, 1);
    let _ = std::fs::remove_dir_all(cache);
}

/// A Draft from `from` to `to`, built.
fn built(from: &str, to: &str, subject: &str) -> muttrs_mail::compose::Built {
    use muttrs_mail::compose::Draft;
    let mut draft = Draft::new(&muttrs_mail::compose::Sender {
        addresses: vec![muttrs_mail::Address {
            name: Some("Tester".into()),
            email: from.into(),
        }],
        signature: None,
    });
    draft.to = vec![muttrs_mail::Address {
        name: None,
        email: to.into(),
    }];
    draft.subject = subject.into();
    draft.body = format!("**{subject}**, in Markdown.\n");
    draft
        .build(&muttrs_mail::compose::message_id(from))
        .unwrap()
}

/// Waits until GreenMail has delivered a Message to `user`'s Inbox, and gives its subjects.
async fn delivered(user: &str) -> Vec<String> {
    let config = tls::client_config(&roots()).unwrap();
    for _ in 0..40 {
        let mut session = Session::connect(
            &endpoint(3993, Tls::Ssl),
            Arc::clone(&config),
            &password(user, "any"),
        )
        .await
        .unwrap();
        let selected = session.select("INBOX").await.unwrap();
        if selected.exists > 0 {
            return session
                .newest(selected.exists, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|envelope| envelope.subject)
                .collect();
        }
        session.logout().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("nothing was delivered to {user}");
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn sending_over_tls_to_greenmail() {
    let (from, to) = (user("smtps-from"), user("smtps-to"));
    let sent = built(&from, &to, "over TLS");
    muttrs_mail::smtp::send(
        &endpoint(3465, Tls::Ssl),
        &roots(),
        &password(&from, "any"),
        &sent,
    )
    .await
    .unwrap();
    assert_eq!(delivered(&to).await, ["over TLS"]);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn sending_with_xoauth2() {
    let (from, to) = (user("xoauth2-from"), user("xoauth2-to"));
    let credentials = Credentials::Bearer {
        username: from.clone(),
        token: "ya29.any".into(),
    };
    muttrs_mail::smtp::send(
        &endpoint(3465, Tls::Ssl),
        &roots(),
        &credentials,
        &built(&from, &to, "with XOAUTH2"),
    )
    .await
    .unwrap();
    assert_eq!(delivered(&to).await, ["with XOAUTH2"]);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn sending_with_starttls_through_dovecot() {
    let to = user("starttls-to");
    let from = "alex@localhost";
    muttrs_mail::smtp::send(
        &endpoint(31587, Tls::Starttls),
        &roots(),
        &password("alex", "secret"),
        &built(from, &to, "with STARTTLS"),
    )
    .await
    .unwrap();
    assert_eq!(delivered(&to).await, ["with STARTTLS"]);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn a_refused_password_is_no_sending() {
    let error = muttrs_mail::smtp::send(
        &endpoint(31587, Tls::Starttls),
        &roots(),
        &password("alex", "wrong"),
        &built("alex@localhost", "nobody@localhost", "refused"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("31587"), "{error}");
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn an_accounts_client_sends_and_keeps_a_copy() {
    let username = user("sender");
    let to = user("sent-to");
    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 31587, tls = \"starttls\" }}",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(None),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    client.send(Request::Send {
        built: Box::new(built(&username, &to, "kept in Sent")),
        answers: None,
    });
    let sent = until(&mut updates, |update| match update {
        Update::Sent { sent, .. } => Some(sent.clone()),
        _ => None,
    })
    .await;
    assert_eq!(sent, Ok(()));
    assert_eq!(delivered(&to).await, ["kept in Sent"]);
    let counts = until(&mut updates, |update| match update {
        Update::Counts { mailbox, counts } if mailbox == "Sent" && counts.messages > 0 => {
            Some(*counts)
        }
        _ => None,
    })
    .await;
    assert_eq!(counts.messages, 1);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn a_servers_own_certificate_trusted_as_it_is() {
    let config = tls::client_config(&pinned()).unwrap();
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        config,
        &password(&user("pinned"), "secret"),
    )
    .await
    .unwrap();
    assert_ne!(session.mailboxes().await.unwrap(), []);

    let (from, to) = (user("pinned-from"), user("pinned-to"));
    muttrs_mail::smtp::send(
        &endpoint(3465, Tls::Ssl),
        &pinned(),
        &password(&from, "any"),
        &built(&from, &to, "pinned"),
    )
    .await
    .unwrap();
    assert_eq!(delivered(&to).await, ["pinned"]);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn another_certificate_than_the_one_trusted_is_refused() {
    // The CA's certificate, pinned, isn't the server's.
    let wrong = Trust::Pinned(ca());
    let error = Session::connect(
        &endpoint(31993, Tls::Ssl),
        tls::client_config(&wrong).unwrap(),
        &password(&user("wrong-pin"), "secret"),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("isn't one of server_certificate's"),
        "{error}"
    );

    let error = muttrs_mail::smtp::send(
        &endpoint(31587, Tls::Starttls),
        &wrong,
        &password("alex", "secret"),
        &built("alex@localhost", "nobody@localhost", "refused"),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, muttrs_mail::smtp::Error::NotPinned(..)),
        "{error}"
    );
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn what_servers_present_is_seen_before_it_is_trusted() {
    let server = tls::certificates(&ca().with_file_name("server.pem")).unwrap();
    for imap in [endpoint(31143, Tls::Starttls), endpoint(3993, Tls::Ssl)] {
        let presented = muttrs_mail::imap::presented_certificates(&imap)
            .await
            .unwrap();
        assert_eq!(presented[0], server[0], "{imap}");
    }
    for smtp in [endpoint(31587, Tls::Starttls), endpoint(3465, Tls::Ssl)] {
        let presented = muttrs_mail::smtp::presented_certificates(&smtp)
            .await
            .unwrap();
        assert_eq!(presented[0], server[0].as_ref(), "{smtp}");
    }
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn older_envelopes_come_page_by_page() {
    let username = user("pages");
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    for number in 1..=350 {
        session
            .append("INBOX", "(\\Seen)", &message(&format!("m{number}")))
            .await
            .unwrap();
    }
    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(None),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    client.send(Request::Open("INBOX".into()));
    let (listed, exists) = until(&mut updates, |update| match update {
        Update::Envelopes {
            envelopes, exists, ..
        } => Some((envelopes.clone(), *exists)),
        _ => None,
    })
    .await;
    assert_eq!((listed.len(), exists), (300, 350));
    assert_eq!(listed[0].subject, "m51", "the newest 300");

    client.send(Request::More("INBOX".into()));
    let listed = until(&mut updates, |update| match update {
        Update::Envelopes { envelopes, .. } if envelopes.len() > 300 => Some(envelopes.clone()),
        _ => None,
    })
    .await;
    assert_eq!(listed.len(), 350);
    assert_eq!(listed[0].subject, "m1");
}

/// Appends Messages about lunch and about a report as `username` at `port`, then searches them through an
/// Account's client.
async fn search(port: u16, tls: Tls, username: &str, password_word: &str) {
    let mut session = Session::connect(
        &endpoint(port, tls),
        tls::client_config(&roots()).unwrap(),
        &password(username, password_word),
    )
    .await
    .unwrap();
    for subject in [
        "lunch on Thursday",
        "the quarterly report",
        "re lunch",
        "coffee \"au lait\"",
    ] {
        session
            .append("INBOX", "()", &message(subject))
            .await
            .unwrap();
    }
    let tls_name = if tls == Tls::Ssl { "ssl" } else { "starttls" };
    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo {password_word}\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = {port}, tls = \"{tls_name}\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(None),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    for (query, wanted) in [
        ("lunch", vec!["lunch on Thursday", "re lunch"]),
        ("quarterly", vec!["the quarterly report"]),
        ("\"au lait\"", vec!["coffee \"au lait\""]),
        ("nothing like it", vec![]),
    ] {
        client.send(Request::Search {
            mailbox: "INBOX".into(),
            query: query.into(),
        });
        let (found, count) = until(&mut updates, |update| match update {
            Update::Found {
                envelopes, count, ..
            } => Some((envelopes.clone(), *count)),
            Update::Failed(failed) => panic!("{failed}"),
            _ => None,
        })
        .await;
        let mut subjects: Vec<String> =
            found.into_iter().map(|envelope| envelope.subject).collect();
        subjects.sort();
        assert_eq!(
            (subjects, count),
            (
                wanted.iter().map(|s| (*s).to_owned()).collect(),
                wanted.len()
            ),
            "{query}"
        );
    }
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn searching_dovecot() {
    search(31143, Tls::Starttls, &user("search"), "secret").await;
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn searching_greenmail() {
    search(3993, Tls::Ssl, &user("search"), "any").await;
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn offline_what_was_listed_and_read_is_still_there() {
    let username = user("offline");
    let cache_root =
        std::env::temp_dir().join(format!("muttrs-offline-test-{}", std::process::id()));
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    session
        .append("INBOX", "()", &message("kept"))
        .await
        .unwrap();
    let account = |port: u16| -> muttrs_mail::Account {
        let config: AccountConfig = toml::from_str(&format!(
            "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
             imap = {{ host = \"localhost\", port = {port}, tls = \"starttls\" }}\n\
             smtp = {{ host = \"localhost\", port = {port}, tls = \"starttls\" }}",
            ca().display().to_string()
        ))
        .unwrap();
        config.resolve().unwrap()
    };
    let spawn = |account| {
        let (sender, updates) = tokio::sync::mpsc::unbounded_channel();
        let sender = Arc::new(Mutex::new(sender));
        let client = Client::spawn(
            account,
            Cache::new(Some(cache_root.clone())),
            Arc::new(move |update| {
                let _ = sender.lock().unwrap().send(update);
            }),
        )
        .unwrap();
        (client, updates)
    };

    // Online: listed, and read.
    let (online, mut updates) = spawn(account(31143));
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    online.send(Request::Open("INBOX".into()));
    let uid = until(&mut updates, |update| match update {
        Update::Envelopes { envelopes, .. } => envelopes.first().map(|envelope| envelope.uid),
        _ => None,
    })
    .await;
    online.send(Request::Body {
        mailbox: "INBOX".into(),
        uid,
    });
    until(&mut updates, |update| {
        matches!(update, Update::Body { .. }).then_some(())
    })
    .await;
    drop(online);

    // Offline, on a port nothing listens on: what the cache keeps.
    let (offline, mut updates) = spawn(account(1));
    let mut next = async || {
        tokio::time::timeout(Duration::from_secs(20), updates.recv())
            .await
            .expect("an update in time")
            .expect("updates")
    };
    loop {
        if let Update::Connection(Connection::Offline(_)) = next().await {
            break;
        }
    }
    offline.send(Request::Open("INBOX".into()));
    offline.send(Request::Body {
        mailbox: "INBOX".into(),
        uid,
    });
    offline.send(Request::Send {
        built: Box::new(built(&username, "nobody@localhost", "stays")),
        answers: None,
    });
    let mut seen = (false, false, false);
    while seen != (true, true, true) {
        match next().await {
            Update::Envelopes { envelopes, .. } => {
                assert_eq!(envelopes[0].subject, "kept");
                seen.0 = true;
            }
            Update::Body { body, .. } => {
                assert!(String::from_utf8_lossy(&body.unwrap()).contains("Hello from kept."));
                seen.1 = true;
            }
            Update::Sent { sent, .. } => {
                assert!(sent.unwrap_err().contains("offline"));
                seen.2 = true;
            }
            _ => {}
        }
    }
    let _ = std::fs::remove_dir_all(cache_root);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn tasks_are_told_started_and_finished_and_what_they_changed_after() {
    let username = user("tasks");
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    for subject in ["a", "b", "c"] {
        session
            .append("INBOX", "()", &message(subject))
            .await
            .unwrap();
    }
    session.create("Trash").await.unwrap();
    // No Archive Mailbox: archiving is refused.
    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}\n[mailboxes]\ntrash = \"Trash\"",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(None),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    client.send(Request::Open("INBOX".into()));
    let uids = until(&mut updates, |update| match update {
        Update::Envelopes { envelopes, .. } if envelopes.len() == 3 => Some(
            envelopes
                .iter()
                .map(|envelope| envelope.uid)
                .collect::<Vec<_>>(),
        ),
        _ => None,
    })
    .await;

    // Asked one after the other, as keys pressed quickly ask them.
    let mailbox = || "INBOX".to_owned();
    let delete = client.send(Request::Delete {
        mailbox: mailbox(),
        uids: vec![uids[0]],
    });
    let archive = client.send(Request::Archive {
        mailbox: mailbox(),
        uids: vec![uids[1]],
    });
    let read = client.send(Request::Body {
        mailbox: mailbox(),
        uid: uids[2],
    });
    let name = |task| {
        [(delete, "delete"), (archive, "archive"), (read, "read")]
            .into_iter()
            .find_map(|(asked, name)| (asked == task).then_some(name))
    };
    // What's told of them, in order, and the Inbox's subjects each time they're told.
    let mut told: Vec<String> = Vec::new();
    while !(told.contains(&"finished archive".to_owned())
        && told.last().is_some_and(|last| last.starts_with("listed")))
    {
        let update = tokio::time::timeout(Duration::from_secs(20), updates.recv())
            .await
            .expect("an update in time")
            .expect("updates");
        match update {
            Update::Started(task) => {
                told.extend(name(task).map(|name| format!("started {name}")));
            }
            Update::Finished(task) => {
                told.extend(name(task).map(|name| format!("finished {name}")));
            }
            Update::Envelopes {
                mailbox, envelopes, ..
            } if mailbox == "INBOX" => {
                let subjects: Vec<&str> = envelopes.iter().map(|e| e.subject.as_str()).collect();
                told.push(format!("listed {}", subjects.join(" ")));
            }
            _ => {}
        }
    }
    let tasks: Vec<&str> = told
        .iter()
        .map(String::as_str)
        .filter(|told| !told.starts_with("listed"))
        .collect();
    let one_at_a_time = |first: &str, second: &str| {
        vec![
            format!("started {first}"),
            format!("finished {first}"),
            format!("started {second}"),
            format!("finished {second}"),
            "started archive".to_owned(),
            "finished archive".to_owned(),
        ]
    };
    // The worker may have taken up the deletion before the rest was asked; else reading goes first.
    assert!(
        tasks == one_at_a_time("delete", "read") || tasks == one_at_a_time("read", "delete"),
        "{told:?}"
    );
    let after = |what: &str| {
        let at = told.iter().position(|told| told == what).unwrap();
        told[at + 1].clone()
    };
    assert_eq!(
        after("finished delete"),
        "listed b c",
        "deleted, as listed since"
    );
    assert_eq!(after("finished archive"), "listed b c", "refused: b stays");
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn resyncing_lists_anew_what_was_kept_wrong() {
    let username = user("resync");
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    session
        .append("INBOX", "()", &message("as it is"))
        .await
        .unwrap();
    let selected = session.select("INBOX").await.unwrap();
    // The cache keeps it under another subject, as a fault would leave it: what's listed is brought up to date by
    // its Flags, and a Message's headers never change, so opening and refreshing keep it so.
    let root = std::env::temp_dir().join(format!("muttrs-resync-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let kept = |subject: &str| {
        let header = format!("Subject: {subject}\r\n\r\n");
        Listed {
            uid_validity: selected.uid_validity,
            exists: 1,
            envelopes: vec![Envelope::read(
                1,
                Flags::default(),
                None,
                0,
                header.as_bytes(),
            )],
        }
    };
    Cache::new(Some(root.clone())).keep_envelopes("INBOX", &kept("kept wrong"));
    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(Some(root.clone())),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    // What's listed when the asked is finished: the Inbox's subjects, as last told.
    let listed_once_finished =
        async |updates: &mut tokio::sync::mpsc::UnboundedReceiver<Update>, task| {
            let mut subjects = Vec::new();
            until(updates, |update| match update {
                Update::Envelopes {
                    mailbox, envelopes, ..
                } if mailbox == "INBOX" => {
                    subjects = envelopes.iter().map(|e| e.subject.clone()).collect();
                    None
                }
                Update::Finished(finished) if *finished == task => Some(()),
                Update::Failed(failed) => panic!("{failed}"),
                _ => None,
            })
            .await;
            subjects
        };
    let open = client.send(Request::Open("INBOX".into()));
    assert_eq!(
        listed_once_finished(&mut updates, open).await,
        ["kept wrong"]
    );
    let refresh = client.send(Request::Refresh);
    assert_eq!(
        listed_once_finished(&mut updates, refresh).await,
        ["kept wrong"]
    );

    let resync = client.send(Request::Resync("INBOX".into()));
    assert_eq!(
        listed_once_finished(&mut updates, resync).await,
        ["as it is"]
    );
    let kept = Cache::new(Some(root.clone())).envelopes("INBOX").unwrap();
    assert_eq!(kept.envelopes[0].subject, "as it is", "kept anew");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn moving_back_undoes_deleting_and_moving() {
    let username = user("undo");
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    for subject in ["kept", "deleted", "moved"] {
        session
            .append("INBOX", "()", &message(subject))
            .await
            .unwrap();
    }
    session.create("Trash").await.unwrap();
    session.create("Work").await.unwrap();
    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}\n[mailboxes]\ntrash = \"Trash\"",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(None),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    until(&mut updates, |update| {
        matches!(update, Update::Mailboxes(_)).then_some(())
    })
    .await;
    client.send(Request::Open("INBOX".into()));
    let listed = until(&mut updates, |update| match update {
        Update::Envelopes { envelopes, .. } if envelopes.len() == 3 => Some(envelopes.clone()),
        _ => None,
    })
    .await;
    let of = |subject: &str| {
        let envelope = listed.iter().find(|e| e.subject == subject).unwrap();
        (envelope.uid, envelope.message_id.clone().unwrap())
    };
    let ((deleted, deleted_id), (moved, moved_id)) = (of("deleted"), of("moved"));
    // What's said once the Task's finished: what it did, or why it didn't.
    let said = async |updates: &mut tokio::sync::mpsc::UnboundedReceiver<Update>, task| {
        let mut said = String::new();
        until(updates, |update| match update {
            Update::Done(text) | Update::Failed(text) => {
                said.clone_from(text);
                None
            }
            Update::Finished(finished) if *finished == task => Some(()),
            _ => None,
        })
        .await;
        said
    };
    let delete = client.send(Request::Delete {
        mailbox: "INBOX".into(),
        uids: vec![deleted],
    });
    assert_eq!(said(&mut updates, delete).await, "Moved a Message to Trash");
    let elsewhere = client.send(Request::Move {
        mailbox: "INBOX".into(),
        uids: vec![moved],
        to: "Work".into(),
    });
    assert_eq!(
        said(&mut updates, elsewhere).await,
        "Moved a Message to Work"
    );
    assert_eq!(session.counts("INBOX").await.unwrap().messages, 1);

    // Undone, the last first.
    let back = client.send(Request::MoveBack {
        went: Went::Mailbox("Work".into()),
        to: "INBOX".into(),
        message_ids: vec![moved_id],
    });
    assert_eq!(
        said(&mut updates, back).await,
        "Moved a Message back to Inbox"
    );
    let back = client.send(Request::MoveBack {
        went: Went::Trash,
        to: "INBOX".into(),
        message_ids: vec![deleted_id.clone()],
    });
    assert_eq!(
        said(&mut updates, back).await,
        "Moved a Message back to Inbox"
    );
    for (mailbox, messages) in [("INBOX", 3), ("Trash", 0), ("Work", 0)] {
        assert_eq!(
            session.counts(mailbox).await.unwrap().messages,
            messages,
            "{mailbox}"
        );
    }

    // What's no longer where it went isn't found there.
    let again = client.send(Request::MoveBack {
        went: Went::Trash,
        to: "INBOX".into(),
        message_ids: vec![deleted_id],
    });
    assert_eq!(
        said(&mut updates, again).await,
        "Not moved back: they're no longer in Trash"
    );
}

/// Waits for what the client says it did, failing if it says it couldn't.
async fn done(updates: &mut tokio::sync::mpsc::UnboundedReceiver<Update>) -> String {
    until(updates, |update| match update {
        Update::Done(done) => Some(done.clone()),
        Update::Failed(failed) => panic!("{failed}"),
        _ => None,
    })
    .await
}

/// The Labels each of the Inbox's Messages has, by subject.
type Labelled = Vec<(String, Vec<String>)>;

/// Waits for the client to list the Inbox's Messages with the Labels `wanted`, in the order listed, failing if it
/// doesn't in a while: their UIDs. It may list them otherwise first, as before finding out what each Label has.
async fn labels_listed(
    updates: &mut tokio::sync::mpsc::UnboundedReceiver<Update>,
    wanted: &Labelled,
) -> Vec<u32> {
    until(updates, |update| match update {
        Update::Envelopes {
            mailbox, envelopes, ..
        } if mailbox == "INBOX" => {
            let listed: Labelled = envelopes
                .iter()
                .map(|envelope| (envelope.subject.clone(), envelope.labels.clone()))
                .collect();
            (listed == *wanted).then(|| envelopes.iter().map(|envelope| envelope.uid).collect())
        }
        _ => None,
    })
    .await
}

#[tokio::test]
#[ignore = "needs scripts/test-servers.sh"]
async fn labels_as_mailboxes_of_their_own_as_protons_bridge_keeps_them() {
    let username = user("labels");
    let mut session = Session::connect(
        &endpoint(31143, Tls::Starttls),
        tls::client_config(&roots()).unwrap(),
        &password(&username, "secret"),
    )
    .await
    .unwrap();
    for subject in ["one", "two"] {
        session
            .append("INBOX", "()", &message(subject))
            .await
            .unwrap();
    }
    session.create("Labels/GH").await.unwrap();
    session.create("Labels/Rust for Linux").await.unwrap();
    // "one" labelled GH before the client connects, as by another.
    session
        .append("Labels/GH", "()", &message("one"))
        .await
        .unwrap();

    let config: AccountConfig = toml::from_str(&format!(
        "name = \"test\"\nemail = \"{username}\"\npassword_command = \"echo secret\"\nca_file = {:?}\n\
         imap = {{ host = \"localhost\", port = 31143, tls = \"starttls\" }}\n\
         smtp = {{ host = \"localhost\", port = 3465 }}",
        ca().display().to_string()
    ))
    .unwrap();
    let (sender, mut updates) = tokio::sync::mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(sender));
    let client = Client::spawn(
        config.resolve().unwrap(),
        Cache::new(None),
        Arc::new(move |update| {
            let _ = sender.lock().unwrap().send(update);
        }),
    )
    .unwrap();
    let labels = until(&mut updates, |update| match update {
        Update::Labels(labels) => Some(labels.clone()),
        _ => None,
    })
    .await;
    let names: Vec<(&str, &str)> = labels
        .iter()
        .map(|label| (label.name.as_str(), label.mailbox.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("GH", "Labels/GH"),
            ("Rust for Linux", "Labels/Rust for Linux")
        ]
    );

    client.send(Request::Open("INBOX".into()));
    let labelled = |one: &[&str], two: &[&str]| -> Labelled {
        let owned = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect();
        vec![
            ("one".to_owned(), owned(one)),
            ("two".to_owned(), owned(two)),
        ]
    };
    let uids = labels_listed(&mut updates, &labelled(&["GH"], &[])).await;
    let (one, two) = (uids[0], uids[1]);
    let label = |uids: &[u32], label: &str, on: bool| Request::Label {
        mailbox: "INBOX".into(),
        uids: uids.to_vec(),
        label: label.into(),
        on,
    };

    // Both labelled GH: "two" copied there, "one" not again.
    client.send(label(&[one, two], "Labels/GH", true));
    assert_eq!(done(&mut updates).await, "Labelled 2 Messages GH");
    labels_listed(&mut updates, &labelled(&["GH"], &["GH"])).await;
    assert_eq!(session.counts("Labels/GH").await.unwrap().messages, 2);

    client.send(label(&[two], "Labels/Rust for Linux", true));
    assert_eq!(
        done(&mut updates).await,
        "Labelled a Message Rust for Linux"
    );
    labels_listed(&mut updates, &labelled(&["GH"], &["GH", "Rust for Linux"])).await;

    // Taken off by expunging the copy, found by its Message-ID; the Message stays in the Inbox.
    client.send(label(&[one], "Labels/GH", false));
    assert_eq!(done(&mut updates).await, "Took GH off a Message");
    labels_listed(&mut updates, &labelled(&[], &["GH", "Rust for Linux"])).await;
    assert_eq!(session.counts("Labels/GH").await.unwrap().messages, 1);
    assert_eq!(session.counts("INBOX").await.unwrap().messages, 2);

    // Labelled by another meanwhile: seen when the Inbox is opened again.
    session
        .append("Labels/GH", "()", &message("one"))
        .await
        .unwrap();
    client.send(Request::Open("INBOX".into()));
    labels_listed(&mut updates, &labelled(&["GH"], &["GH", "Rust for Linux"])).await;
    session.logout().await;
}
