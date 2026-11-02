//! IMAP against real servers: GreenMail and Dovecot, in containers that `scripts/test-servers.sh up` starts. Ignored
//! unless asked for:
//!
//! ```sh
//! MUTTRS_TEST_CA=target/test-servers/ca.pem cargo test -p muttrs-mail --test servers -- --ignored
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use muttrs_mail::account::{Endpoint, Tls};
use muttrs_mail::cache::Cache;
use muttrs_mail::client::{Client, Connection, Request, Update};
use muttrs_mail::imap::{Credentials, Error, Session};
use muttrs_mail::{AccountConfig, Message, Role, tls};

fn ca() -> std::path::PathBuf {
    std::env::var_os("MUTTRS_TEST_CA")
        .expect("MUTTRS_TEST_CA names the test servers' CA: see scripts/test-servers.sh")
        .into()
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
    let config = tls::client_config(Some(&ca())).unwrap();
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
    let config = tls::client_config(Some(&ca())).unwrap();
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
    let config = tls::client_config(Some(&ca())).unwrap();
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
    let config = tls::client_config(Some(&ca())).unwrap();
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
    let config = tls::client_config(Some(&ca())).unwrap();
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
    let config = tls::client_config(Some(&ca())).unwrap();
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
    let config = tls::client_config(None).unwrap();
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
    wanted: impl Fn(&Update) -> Option<T>,
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
        tls::client_config(Some(&ca())).unwrap(),
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
