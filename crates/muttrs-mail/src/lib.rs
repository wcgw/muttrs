//! muttrs' Accounts: IMAP, SMTP, TLS, OAuth2, secrets and MIME.

pub mod account;
pub mod cache;
pub mod client;
pub mod compose;
pub mod envelope;
pub mod imap;
pub mod label;
pub mod mailbox;
pub mod mbox;
pub mod message;
pub mod oauth2;
pub mod secret;
pub mod smtp;
pub mod thread;
pub mod tls;

pub use account::{Account, AccountConfig};
pub use envelope::{Envelope, Flags};
pub use label::Label;
pub use mailbox::{Counts, Mailbox, Role};
pub use message::{Address, Message, Part, TextFormat};
