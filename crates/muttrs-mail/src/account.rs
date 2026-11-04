//! Accounts: as the configuration writes them, and as muttrs connects with them, presets applied.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::compose::Sender;
use crate::message::Address;
use crate::tls::Trust;

/// An Account as the configuration writes it, under `[[account]]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    /// What the user calls it: in the sidebar, in `muttrs login NAME`, and in the keyring.
    pub name: String,
    /// The address mail is sent from.
    pub email: String,
    /// The name sent with it.
    pub display_name: Option<String>,
    /// A provider whose servers and quirks muttrs knows: `"gmail"` or `"outlook"`.
    pub provider: Option<Provider>,
    /// Where mail is read: required without a provider.
    pub imap: Option<ServerConfig>,
    /// Where mail is sent: required without a provider.
    pub smtp: Option<ServerConfig>,
    /// The user name to sign in with; the address by default.
    pub username: Option<String>,
    /// How to sign in: `"password"`, or `"oauth2"` (the default for providers that want it).
    pub auth: Option<AuthKind>,
    /// A command that prints the password, as `pass show mail/work`; without one, the password is in the keyring.
    pub password_command: Option<String>,
    /// OAuth2's client and endpoints; a provider fills in the endpoints.
    pub oauth2: Option<OAuth2Config>,
    /// A PEM file of CA certificates to trust besides the platform's, as for servers whose certificates a CA of
    /// one's own signs.
    pub ca_file: Option<PathBuf>,
    /// A PEM file of the certificates the servers present, to trust as they are and to trust alone: for a server
    /// with a certificate of its own, self-signed, as mail bridges have (Proton Mail Bridge's, say).
    /// `muttrs trust NAME` saves them.
    pub server_certificate: Option<PathBuf>,
    /// Whether to put a copy of what's sent in the Sent Mailbox; not for Gmail, which does it itself.
    pub copy_to_sent: Option<bool>,
    /// Mailboxes by Role, where the server doesn't say (SPECIAL-USE), as `{ archive = "Archive" }`.
    #[serde(default)]
    pub mailboxes: RoleNames,
    /// Other addresses it sends as, as `"Alex <alex@work.example>"`: a reply is from the one it was sent to.
    #[serde(default)]
    pub identities: Vec<String>,
    /// What goes under a Draft, after a `-- ` line.
    pub signature: Option<String>,
    /// A file holding the signature, rather than writing it here.
    pub signature_file: Option<PathBuf>,
    /// The Account's address book on a CardDAV server, to complete addresses from.
    pub carddav: Option<CardDavConfig>,
}

/// An address book on a CardDAV server, as the configuration says.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardDavConfig {
    /// The address book's URL: the collection its cards are in.
    pub url: String,
    /// The user name to sign in with: the Account's, if it's not given.
    pub username: Option<String>,
    /// A command that prints the password: the Account's password, if it's not given.
    pub password_command: Option<String>,
}

/// A server as the configuration writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Its host name or address.
    pub host: String,
    /// The port; by default the one for `tls`.
    pub port: Option<u16>,
    /// How the connection is secured; implicit TLS by default.
    #[serde(default)]
    pub tls: Tls,
}

/// How a connection is secured.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tls {
    /// TLS from the start (IMAPS on 993, SMTPS on 465).
    #[default]
    Ssl,
    /// Plain text, upgraded with STARTTLS before signing in (IMAP on 143, submission on 587).
    Starttls,
    /// None at all: only to the loopback address, for testing.
    None,
}

/// A provider whose servers and quirks muttrs knows.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Gmail, through Google's servers and OAuth2.
    Gmail,
    /// Outlook, through Microsoft's servers and OAuth2.
    Outlook,
}

/// How the configuration says to sign in.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    /// With a password, from the keyring or a command.
    Password,
    /// With OAuth2 (XOAUTH2), after signing in once in the browser with `muttrs login`.
    OAuth2,
}

/// OAuth2's settings. Each user registers a client of their own with the provider: muttrs has none.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth2Config {
    /// The client's ID, as the provider issued it.
    pub client_id: String,
    /// The client's secret, which Google issues even to desktop clients (where it isn't secret).
    pub client_secret: Option<String>,
    /// A command printing the client's secret, rather than writing it here.
    pub client_secret_command: Option<String>,
    /// The provider's authorization endpoint, where the user signs in: required without a provider.
    pub auth_url: Option<String>,
    /// The provider's token endpoint, which gives the tokens: required without a provider.
    pub token_url: Option<String>,
    /// The scopes to ask for: required without a provider.
    pub scopes: Option<Vec<String>>,
}

/// Mailboxes by Role, by name.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoleNames {
    /// The name of the Sent Mailbox.
    pub sent: Option<String>,
    /// The name of the Drafts Mailbox.
    pub drafts: Option<String>,
    /// The name of the Trash Mailbox.
    pub trash: Option<String>,
    /// The name of the Junk Mailbox.
    pub junk: Option<String>,
    /// The name of the Archive Mailbox.
    pub archive: Option<String>,
}

/// Where a server is, and how to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// Its host name or address.
    pub host: String,
    /// The port to connect to.
    pub port: u16,
    /// How the connection is secured.
    pub tls: Tls,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tls = match self.tls {
            Tls::Ssl => "TLS",
            Tls::Starttls => "STARTTLS",
            Tls::None => "no TLS",
        };
        write!(f, "{}:{} ({tls})", self.host, self.port)
    }
}

/// How an Account signs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    /// With a password, from the keyring or a command.
    Password(PasswordSource),
    /// With OAuth2 access tokens (XOAUTH2), refreshed with a refresh token from the keyring.
    OAuth2(OAuth2),
}

/// Where an Account's password comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasswordSource {
    /// The system keyring, where `muttrs login` stores it.
    Keyring,
    /// The first line this command prints, as `pass show mail/work`.
    Command(String),
}

/// OAuth2's client and endpoints, complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuth2 {
    /// The client's ID, as the provider issued it.
    pub client_id: String,
    /// The client's secret, if it has one.
    pub client_secret: Option<Secret>,
    /// The provider's authorization endpoint, where the user signs in.
    pub auth_url: String,
    /// The provider's token endpoint, which gives the tokens.
    pub token_url: String,
    /// The scopes signing in asks for.
    pub scopes: Vec<String>,
}

/// A secret written in the configuration, or printed by a command.
#[derive(Clone, PartialEq, Eq)]
pub enum Secret {
    /// Written in the configuration.
    Value(String),
    /// Printed by this command.
    Command(String),
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(_) => f.write_str("Secret(…)"),
            Self::Command(command) => write!(f, "Secret(command {command:?})"),
        }
    }
}

/// An Account, ready to connect with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// What the user calls it, and what the keyring knows it by.
    pub name: String,
    /// The address mail is sent from.
    pub email: String,
    /// The name sent with the address.
    pub display_name: Option<String>,
    /// The user name to sign in with.
    pub username: String,
    /// The IMAP server mail is read from.
    pub imap: Endpoint,
    /// The SMTP server mail is sent through.
    pub smtp: Endpoint,
    /// How it signs in, to both.
    pub auth: Auth,
    /// How its servers' certificates are trusted.
    pub trust: Trust,
    /// Whether to put a copy of what's sent in the Sent Mailbox.
    pub copy_to_sent: bool,
    /// Mailboxes by Role, where the configuration names them.
    pub roles: RoleNames,
    /// Whether it's on Gmail, whose labels are Mailboxes and which archives out of the Inbox.
    pub gmail: bool,
    /// Its other addresses, which it sends as too.
    pub identities: Vec<Address>,
    /// What goes under its Drafts.
    pub signature: Option<String>,
    /// Its address book on a CardDAV server, if it has one.
    pub carddav: Option<crate::carddav::CardDav>,
}

impl Account {
    /// Who its Drafts are from.
    #[must_use]
    pub fn sender(&self) -> Sender {
        let own = Address {
            name: self.display_name.clone(),
            email: self.email.clone(),
        };
        Sender {
            addresses: std::iter::once(own)
                .chain(self.identities.iter().cloned())
                .collect(),
            signature: self.signature.clone(),
        }
    }
}

/// What's wrong with an Account's configuration.
#[derive(Debug, thiserror::Error)]
#[error("account {account:?}: {problem}")]
pub struct Invalid {
    account: String,
    problem: String,
}

impl AccountConfig {
    /// The Account this configuration describes, its provider's settings filled in.
    ///
    /// # Errors
    ///
    /// If a server is missing, OAuth2 lacks its client, or unencrypted connections go anywhere but the loopback.
    pub fn resolve(self) -> Result<Account, Invalid> {
        let invalid = |problem: String| Invalid {
            account: self.name.clone(),
            problem,
        };
        let preset = self.provider.map(Preset::of);
        let server = |config: &Option<ServerConfig>,
                      preset: Option<&Endpoint>,
                      what: &str,
                      ports: (u16, u16)| {
            match (config, preset) {
                (Some(server), _) => {
                    let port = server.port.unwrap_or(match server.tls {
                        Tls::Ssl => ports.0,
                        Tls::Starttls | Tls::None => ports.1,
                    });
                    if server.tls == Tls::None && !is_loopback(&server.host) {
                        return Err(invalid(format!(
                            "{what} has tls = \"none\" to {}: unencrypted connections only go to the loopback address",
                            server.host
                        )));
                    }
                    Ok(Endpoint {
                        host: server.host.clone(),
                        port,
                        tls: server.tls,
                    })
                }
                (None, Some(preset)) => Ok(preset.clone()),
                (None, None) => Err(invalid(format!(
                    "{what} = {{ host = … }} is needed without a provider"
                ))),
            }
        };
        let imap = server(
            &self.imap,
            preset.as_ref().map(|p| &p.imap),
            "imap",
            (993, 143),
        )?;
        let smtp = server(
            &self.smtp,
            preset.as_ref().map(|p| &p.smtp),
            "smtp",
            (465, 587),
        )?;

        let kind = self.auth.unwrap_or(match preset {
            Some(_) => AuthKind::OAuth2,
            None => AuthKind::Password,
        });
        let auth = match kind {
            AuthKind::Password => Auth::Password(
                self.password_command
                    .clone()
                    .map_or(PasswordSource::Keyring, PasswordSource::Command),
            ),
            AuthKind::OAuth2 => {
                let Some(oauth2) = self.oauth2.clone() else {
                    return Err(invalid(
                        "OAuth2 needs [account.oauth2] with the client_id (and client_secret) of a client you \
                         register with the provider; or set auth = \"password\" for an app password"
                            .into(),
                    ));
                };
                let preset_oauth2 = preset.as_ref().map(|preset| &preset.oauth2);
                let auth_url = oauth2
                    .auth_url
                    .or_else(|| preset_oauth2.map(|p| p.0.to_owned()))
                    .ok_or_else(|| {
                        invalid("oauth2.auth_url is needed without a provider".into())
                    })?;
                let token_url = oauth2
                    .token_url
                    .or_else(|| preset_oauth2.map(|p| p.1.to_owned()))
                    .ok_or_else(|| {
                        invalid("oauth2.token_url is needed without a provider".into())
                    })?;
                let scopes = oauth2
                    .scopes
                    .or_else(|| {
                        preset_oauth2.map(|p| p.2.iter().map(|s| (*s).to_owned()).collect())
                    })
                    .ok_or_else(|| invalid("oauth2.scopes are needed without a provider".into()))?;
                let client_secret = match (oauth2.client_secret, oauth2.client_secret_command) {
                    (Some(_), Some(_)) => {
                        return Err(invalid(
                            "oauth2 takes client_secret or client_secret_command, not both".into(),
                        ));
                    }
                    (Some(value), None) => Some(Secret::Value(value)),
                    (None, Some(command)) => Some(Secret::Command(command)),
                    (None, None) => None,
                };
                Auth::OAuth2(OAuth2 {
                    client_id: oauth2.client_id,
                    client_secret,
                    auth_url,
                    token_url,
                    scopes,
                })
            }
        };
        let trust = match (self.ca_file, self.server_certificate) {
            (Some(_), Some(_)) => {
                return Err(invalid(
                    "ca_file and server_certificate don't go together: one trusts the certificates a CA signs, \
                     the other a server's own certificate"
                        .into(),
                ));
            }
            (ca_file, None) => Trust::Roots {
                ca_file: ca_file.map(|path| home(&path)),
            },
            (None, Some(certificate)) => Trust::Pinned(home(&certificate)),
        };
        let (identities, signature) =
            sending_as(&self.identities, self.signature, self.signature_file).map_err(invalid)?;
        let gmail = self.provider == Some(Provider::Gmail)
            || imap.host.eq_ignore_ascii_case("imap.gmail.com");
        let username = self.username.unwrap_or_else(|| self.email.clone());
        let carddav = self
            .carddav
            .map(|book| address_book(book, &username, &auth))
            .transpose()
            .map_err(invalid)?;
        Ok(Account {
            username,
            name: self.name,
            email: self.email,
            display_name: self.display_name,
            imap,
            smtp,
            auth,
            trust,
            // Gmail files what its SMTP server sends in Sent itself: a copy would be a duplicate.
            copy_to_sent: self.copy_to_sent.unwrap_or(!gmail),
            roles: self.mailboxes,
            gmail,
            identities,
            signature,
            carddav,
        })
    }
}

/// The address book `book` says, signed in to as `username` with `auth`'s password unless it says its own; or what's
/// wrong with it.
fn address_book(
    book: CardDavConfig,
    username: &str,
    auth: &Auth,
) -> Result<crate::carddav::CardDav, String> {
    let password = match (book.password_command, auth) {
        (Some(command), _) => PasswordSource::Command(command),
        (None, Auth::Password(source)) => source.clone(),
        (None, Auth::OAuth2(_)) => {
            return Err(
                "carddav needs a password_command: the Account signs in with OAuth2".into(),
            );
        }
    };
    Ok(crate::carddav::CardDav {
        url: book.url,
        username: book.username.unwrap_or_else(|| username.to_owned()),
        password,
    })
}

/// The addresses `identities` say, and the signature, written or in a file; or what's wrong with them.
fn sending_as(
    identities: &[String],
    signature: Option<String>,
    signature_file: Option<PathBuf>,
) -> Result<(Vec<Address>, Option<String>), String> {
    let mut addresses = Vec::new();
    for identity in identities {
        addresses.extend(
            crate::compose::parse_addresses(identity)
                .map_err(|_| format!("identities: {identity:?} isn't an address"))?,
        );
    }
    let signature = match (signature, signature_file) {
        (Some(_), Some(_)) => return Err("signature and signature_file don't go together".into()),
        (Some(signature), None) => Some(signature),
        (None, Some(path)) => {
            let path = home(&path);
            Some(
                std::fs::read_to_string(&path).map_err(|error| {
                    format!("reading signature_file {}: {error}", path.display())
                })?,
            )
        }
        (None, None) => None,
    };
    Ok((addresses, signature))
}

/// `path`, with a leading `~/` meaning the home directory.
fn home(path: &std::path::Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) if !home.is_empty() => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

/// Whether `host` is the loopback address, where nothing's sent over the network.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// What muttrs knows of a provider.
struct Preset {
    imap: Endpoint,
    smtp: Endpoint,
    /// Its authorization and token endpoints, and the scopes mail takes.
    oauth2: (&'static str, &'static str, &'static [&'static str]),
}

impl Preset {
    fn of(provider: Provider) -> Self {
        let endpoint = |host: &str, port, tls| Endpoint {
            host: host.to_owned(),
            port,
            tls,
        };
        match provider {
            Provider::Gmail => Self {
                imap: endpoint("imap.gmail.com", 993, Tls::Ssl),
                smtp: endpoint("smtp.gmail.com", 465, Tls::Ssl),
                oauth2: (
                    "https://accounts.google.com/o/oauth2/v2/auth",
                    "https://oauth2.googleapis.com/token",
                    &["https://mail.google.com/"],
                ),
            },
            Provider::Outlook => Self {
                imap: endpoint("outlook.office365.com", 993, Tls::Ssl),
                smtp: endpoint("smtp.office365.com", 587, Tls::Starttls),
                oauth2: (
                    "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                    "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                    &[
                        "https://outlook.office.com/IMAP.AccessAsUser.All",
                        "https://outlook.office.com/SMTP.Send",
                        "offline_access",
                    ],
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(toml_text: &str) -> Result<Account, Invalid> {
        let config: AccountConfig = toml::from_str(toml_text).expect("valid TOML");
        config.resolve()
    }

    #[test]
    fn gmail_needs_its_client_and_is_filled_in() {
        let error =
            account("name = \"g\"\nemail = \"a@gmail.com\"\nprovider = \"gmail\"").unwrap_err();
        assert!(error.to_string().contains("client_id"), "{error}");

        let gmail = account(
            "name = \"g\"\nemail = \"a@gmail.com\"\nprovider = \"gmail\"\n[oauth2]\nclient_id = \"id\"\nclient_secret = \"s\"",
        )
        .unwrap();
        assert_eq!(gmail.imap.to_string(), "imap.gmail.com:993 (TLS)");
        assert_eq!(gmail.smtp.to_string(), "smtp.gmail.com:465 (TLS)");
        assert!(gmail.gmail && !gmail.copy_to_sent);
        assert_eq!(gmail.username, "a@gmail.com");
        let Auth::OAuth2(oauth2) = gmail.auth else {
            panic!("OAuth2")
        };
        assert_eq!(oauth2.scopes, ["https://mail.google.com/"]);
        assert_eq!(oauth2.client_secret, Some(Secret::Value("s".into())));
    }

    #[test]
    fn gmail_with_an_app_password() {
        let gmail = account(
            "name = \"g\"\nemail = \"a@gmail.com\"\nprovider = \"gmail\"\nauth = \"password\"\npassword_command = \"pass gmail\"",
        )
        .unwrap();
        assert_eq!(
            gmail.auth,
            Auth::Password(PasswordSource::Command("pass gmail".into()))
        );
    }

    #[test]
    fn servers_of_ones_own_take_their_ports_from_their_tls() {
        let own = account(
            "name = \"o\"\nemail = \"a@example.com\"\nimap = { host = \"mail.example.com\", tls = \"starttls\" }\n\
             smtp = { host = \"mail.example.com\" }",
        )
        .unwrap();
        assert_eq!(own.imap.port, 143);
        assert_eq!(own.smtp.port, 465);
        assert_eq!(own.auth, Auth::Password(PasswordSource::Keyring));
        assert!(own.copy_to_sent && !own.gmail);
    }

    #[test]
    fn a_servers_own_certificate_is_trusted_alone() {
        let bridge = account(
            "name = \"bridge\"\nemail = \"a@proton.me\"\nserver_certificate = \"/b/cert.pem\"\n\
             imap = { host = \"127.0.0.1\", port = 1143, tls = \"starttls\" }\n\
             smtp = { host = \"127.0.0.1\", port = 1025, tls = \"starttls\" }",
        )
        .unwrap();
        assert_eq!(bridge.trust, Trust::Pinned("/b/cert.pem".into()));
        let both = account(
            "name = \"b\"\nemail = \"a@x\"\nserver_certificate = \"c.pem\"\nca_file = \"ca.pem\"\n\
             imap = { host = \"x\" }\nsmtp = { host = \"x\" }",
        )
        .unwrap_err();
        assert!(both.to_string().contains("don't go together"), "{both}");
    }

    #[test]
    fn identities_and_signatures() {
        let account = account(
            "name = \"o\"\nemail = \"a@example.com\"\ndisplay_name = \"Ann\"\nsignature = \"Ann\"\n\
             identities = [\"Ann at work <ann@work.example>\"]\n\
             imap = { host = \"x\" }\nsmtp = { host = \"x\" }",
        )
        .unwrap();
        let sender = account.sender();
        let emails: Vec<&str> = sender.addresses.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(emails, ["a@example.com", "ann@work.example"]);
        assert_eq!(sender.addresses[1].name.as_deref(), Some("Ann at work"));
        assert_eq!(sender.signature.as_deref(), Some("Ann"));
        let wrong = account_error(
            "name = \"o\"\nemail = \"a@x\"\nidentities = [\"nobody\"]\nimap = { host = \"x\" }\nsmtp = { host = \"x\" }",
        );
        assert!(wrong.contains("isn't an address"), "{wrong}");
    }

    fn account_error(toml_text: &str) -> String {
        account(toml_text).unwrap_err().to_string()
    }

    #[test]
    fn plain_text_only_to_the_loopback() {
        let error = account(
            "name = \"o\"\nemail = \"a@example.com\"\nimap = { host = \"mail.example.com\", tls = \"none\" }\n\
             smtp = { host = \"localhost\", tls = \"none\" }",
        )
        .unwrap_err();
        assert!(error.to_string().contains("loopback"));
        assert!(
            account(
                "name = \"o\"\nemail = \"a@example.com\"\nimap = { host = \"127.0.0.1\", port = 3143, tls = \"none\" }\n\
                 smtp = { host = \"localhost\", tls = \"none\" }",
            )
            .is_ok()
        );
    }
}
