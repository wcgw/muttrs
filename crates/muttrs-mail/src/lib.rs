//! muttrs' Accounts: IMAP, SMTP, TLS, OAuth2, secrets and MIME.

pub mod account;
pub mod cache;
pub mod client;
pub mod envelope;
pub mod imap;
pub mod mailbox;
pub mod message;
pub mod oauth2;
pub mod secret;
pub mod tls;

pub use account::{Account, AccountConfig};
pub use envelope::{Envelope, Flags};
pub use mailbox::{Counts, Mailbox, Role};
pub use message::{Address, Message, Part, TextFormat};
