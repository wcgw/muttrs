//! Contacts from a CardDAV address book (RFC 6352): the names and addresses of its cards, fetched with one REPORT,
//! for completing addresses with.

use std::time::Duration;

use crate::account::PasswordSource;
use crate::message::Address;
use crate::secret;

/// An Account's address book on a CardDAV server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardDav {
    /// The address book's URL: the collection its cards are in.
    pub url: String,
    /// The user name to sign in with.
    pub username: String,
    /// Where the password comes from.
    pub password: PasswordSource,
}

/// Why the contacts couldn't be had.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// It couldn't be reached, or what it said couldn't be read.
    #[error("reaching the address book: {0}")]
    Http(#[from] reqwest::Error),
    /// It answered, but not with the cards: wrong credentials, or no address book at the URL.
    #[error("the address book answered {0}")]
    Status(reqwest::StatusCode),
    /// The password couldn't be had.
    #[error("its password: {0}")]
    Password(#[from] secret::Error),
}

/// What's asked of the address book: every card's data.
const QUERY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<C:addressbook-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">
  <D:prop><C:address-data/></D:prop>
</C:addressbook-query>"#;

/// The names and addresses of `book`'s cards, the Account's named `account` (by which the keyring keeps its
/// password): one each address a card has.
///
/// # Errors
///
/// If the password can't be had, the server can't be reached, or it refuses.
///
/// # Panics
///
/// Never: `REPORT` is a method's name, as HTTP has them.
pub async fn contacts(book: &CardDav, account: &str) -> Result<Vec<Address>, Error> {
    let password = match &book.password {
        PasswordSource::Keyring => secret::read(account, secret::Kind::Password).await?,
        PasswordSource::Command(command) => secret::from_command(command).await?,
    };
    let report = reqwest::Method::from_bytes(b"REPORT").expect("a method's name");
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?
        .request(report, &book.url)
        .basic_auth(&book.username, Some(password))
        .header("Depth", "1")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/xml; charset=utf-8",
        )
        .body(QUERY)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(Error::Status(response.status()));
    }
    let multistatus = response.text().await?;
    Ok(address_data(&multistatus)
        .iter()
        .flat_map(|card| addresses_of(card))
        .collect())
}

/// The cards a REPORT's multistatus holds: what each `address-data` element has, whatever its namespace's prefix.
fn address_data(xml: &str) -> Vec<String> {
    let mut cards = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        let name = tag.split_whitespace().next().unwrap_or_default();
        let local = name.rsplit(':').next().unwrap_or_default();
        rest = &rest[end + 1..];
        if local != "address-data" || tag.ends_with('/') {
            continue;
        }
        let close = format!("</{name}>");
        let Some(content_end) = rest.find(&close) else {
            break;
        };
        cards.push(unescaped(&rest[..content_end]));
        rest = &rest[content_end + close.len()..];
    }
    cards
}

/// XML text, its entities and CDATA sections read.
fn unescaped(text: &str) -> String {
    if let Some(inner) = text
        .trim()
        .strip_prefix("<![CDATA[")
        .and_then(|inner| inner.strip_suffix("]]>"))
    {
        return inner.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let character = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                })
                .and_then(char::from_u32),
        };
        if let Some(character) = character {
            out.push(character);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The addresses of a vCard, under its formatted name (`FN`).
fn addresses_of(card: &str) -> Vec<Address> {
    // Lines folded onto the next start it with a space or a tab.
    let unfolded = card
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    let mut name = None;
    let mut emails = Vec::new();
    for line in unfolded.lines() {
        let Some((property, value)) = line.split_once(':') else {
            continue;
        };
        // `item1.EMAIL;TYPE=work`: a group before the name, parameters after it.
        let property = property.split(';').next().unwrap_or_default();
        let property = property.rsplit('.').next().unwrap_or_default();
        let value = value.trim();
        if property.eq_ignore_ascii_case("FN") {
            name = Some(
                value
                    .replace("\\,", ",")
                    .replace("\\;", ";")
                    .replace("\\\\", "\\"),
            );
        } else if property.eq_ignore_ascii_case("EMAIL") && value.contains('@') {
            emails.push(value.trim_start_matches("mailto:").to_owned());
        }
    }
    let name = name.filter(|name| !name.trim().is_empty());
    emails
        .into_iter()
        .map(|email| Address {
            name: name.clone(),
            email,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const MULTISTATUS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response><d:href>/ann.vcf</d:href><d:propstat><d:prop>
    <card:address-data>BEGIN:VCARD&#13;
VERSION:3.0&#13;
FN:Ann Example&#13;
item1.EMAIL;TYPE=INTERNET:ann@example.com&#13;
EMAIL;TYPE=work:ann@work.exa&#13;
 mple&#13;
END:VCARD&#13;
</card:address-data>
  </d:prop></d:propstat></d:response>
  <d:response><d:href>/bob.vcf</d:href><d:propstat><d:prop>
    <card:address-data><![CDATA[BEGIN:VCARD
FN:Smith\, Bob & Co
EMAIL:bob@example.com
END:VCARD]]></card:address-data>
  </d:prop></d:propstat></d:response>
  <d:response><d:href>/none.vcf</d:href><d:propstat><d:prop>
    <card:address-data>BEGIN:VCARD
FN:No Mail
TEL:123
END:VCARD</card:address-data>
  </d:prop></d:propstat></d:response>
</d:multistatus>"#;

    fn emails(addresses: &[Address]) -> Vec<String> {
        addresses.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn cards_are_read_from_the_multistatus_whatever_their_prefix() {
        let cards = address_data(MULTISTATUS);
        assert_eq!(cards.len(), 3);
        let addresses: Vec<Address> = cards.iter().flat_map(|card| addresses_of(card)).collect();
        assert_eq!(
            emails(&addresses),
            [
                "Ann Example <ann@example.com>",
                "Ann Example <ann@work.example>",
                "\"Smith, Bob & Co\" <bob@example.com>"
            ]
        );
    }

    #[tokio::test]
    async fn the_address_book_is_asked_for_every_card_signed_in() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = MockServer::start().await;
        Mock::given(method("REPORT"))
            .and(header("Depth", "1"))
            .and(header("Authorization", "Basic YW5uOnNlY3JldA=="))
            .respond_with(ResponseTemplate::new(207).set_body_string(MULTISTATUS))
            .mount(&server)
            .await;
        let book = CardDav {
            url: format!("{}/contacts/", server.uri()),
            username: "ann".into(),
            password: PasswordSource::Command("echo secret".into()),
        };
        let contacts = contacts(&book, "test").await.unwrap();
        assert_eq!(contacts.len(), 3);
        let wrong = CardDav {
            password: PasswordSource::Command("echo wrong".into()),
            ..book
        };
        assert!(matches!(
            super::contacts(&wrong, "test").await,
            Err(Error::Status(status)) if status == 404
        ));
    }
}
