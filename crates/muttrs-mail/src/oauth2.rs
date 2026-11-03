//! OAuth2 for XOAUTH2: the authorization code flow with PKCE, in the browser, redirected to a loopback port
//! (RFC 8252), and access tokens refreshed with the refresh token it gives.
//!
//! A browser on another machine than muttrs (over SSH) can't reach that port: the address it ends at, which has the
//! code, is pasted instead. Google has done away with codes shown to copy ("out of band"), and its device flow,
//! for devices without a browser, doesn't allow Gmail's scope.

use std::fmt;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use ring::rand::SecureRandom as _;
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::account::OAuth2;

/// How long the user has to sign in, in the browser.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Why signing in with OAuth2, or refreshing its access token, failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// When the browser's redirect can't be had: the loopback port can't be listened on, or the redirect brings no
    /// code.
    #[error("waiting for the browser: {0}")]
    Redirect(String),
    /// When the provider refuses, in the browser or at its token endpoint: why, as it says.
    #[error("the provider refused: {0}")]
    Refused(String),
    /// When the token endpoint can't be reached, or its answer can't be read.
    #[error("reaching the provider: {0}")]
    Http(#[from] reqwest::Error),
    /// When signing in gives no refresh token, without which muttrs can't sign in again later.
    #[error(
        "the provider gave no refresh token; revoke muttrs' access in the account's settings and sign in again"
    )]
    NoRefreshToken,
    /// When the user doesn't sign in, in the browser, in time.
    #[error("signing in took too long")]
    TimedOut,
    /// When what's pasted isn't the address the browser ended at: another can be pasted.
    #[error("that isn't the address the browser ended at: copy all of it, from its address bar")]
    NotTheRedirect,
}

/// What the token endpoint gives.
#[derive(Debug)]
pub struct Tokens {
    /// The access token, for XOAUTH2.
    pub access_token: String,
    /// When the access token stops working, by the wall clock: an [`Instant`](std::time::Instant) doesn't count the
    /// time the computer sleeps, after which a token would seem good for longer than it is.
    pub expires: SystemTime,
    /// A new refresh token, if the provider gave one (it always does when signing in; some rotate it).
    pub refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Random bytes, base64url.
fn random(bytes: usize) -> String {
    let mut buffer = vec![0; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut buffer)
        .expect("the system's random numbers");
    URL_SAFE_NO_PAD.encode(buffer)
}

/// PKCE's challenge for `verifier` (S256).
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(
        &ring::digest::SHA256,
        verifier.as_bytes(),
    ))
}

/// Signing in, in the browser: the provider's page to send the browser to, and a loopback port it comes back to with
/// the code that's exchanged for the tokens.
pub struct Authorization {
    /// The provider's page, where the user signs in.
    url: String,
    /// The loopback address the browser comes back to.
    redirect: String,
    /// What the browser comes back with, to tell this sign-in's answer from any other request.
    state: String,
    /// PKCE's verifier, which the code is exchanged with.
    verifier: String,
    listener: TcpListener,
    token_url: String,
    client_id: String,
}

impl fmt::Debug for Authorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authorization")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Authorization {
    /// Starts signing in as `login_hint` with `oauth2`'s client: listens on a loopback port for the browser to come
    /// back to, for as long as this lives.
    ///
    /// # Errors
    ///
    /// If no loopback port can be listened on, or the provider's authorization endpoint isn't a URL.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime, whose reactor the port is listened on with.
    pub fn start(oauth2: &OAuth2, login_hint: &str) -> Result<Self, Error> {
        // Bound at once, without waiting: binding a loopback port doesn't block.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| {
                listener.set_nonblocking(true)?;
                TcpListener::from_std(listener)
            })
            .map_err(|error| Error::Redirect(error.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|error| Error::Redirect(error.to_string()))?
            .port();
        let redirect = format!("http://127.0.0.1:{port}");
        let verifier = random(48);
        let state = random(16);
        let scopes = oauth2.scopes.join(" ");
        let url = Url::parse_with_params(
            &oauth2.auth_url,
            [
                ("response_type", "code"),
                ("client_id", oauth2.client_id.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("scope", scopes.as_str()),
                ("state", state.as_str()),
                ("code_challenge", challenge(&verifier).as_str()),
                ("code_challenge_method", "S256"),
                // Google: a refresh token, every time.
                ("access_type", "offline"),
                ("prompt", "consent"),
                ("login_hint", login_hint),
            ],
        )
        .map_err(|error| Error::Redirect(format!("the authorization URL: {error}")))?;
        Ok(Self {
            url: url.into(),
            redirect,
            state,
            verifier,
            listener,
            token_url: oauth2.token_url.clone(),
            client_id: oauth2.client_id.clone(),
        })
    }

    /// The provider's page, to open in the browser and sign in at.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The tokens signing in gives, exchanged (with `client_secret`, if the client has one) for the code the browser
    /// comes back to the loopback port with, or for the code in an address `pasted` gives: the one the browser ended
    /// at, if it couldn't come back. Blank lines pasted are passed over; once `pasted` closes, only the browser is
    /// waited for.
    ///
    /// # Errors
    ///
    /// [`Error::NotTheRedirect`] if what's pasted isn't the address the browser ended at: called again, this waits
    /// on. Otherwise, if the user doesn't sign in in time, or the provider refuses, can't be reached, or gives no
    /// refresh token.
    ///
    /// # Cancel safety
    ///
    /// Cancelling it gives up on the code, if it came.
    pub async fn tokens(
        &self,
        client_secret: Option<&str>,
        pasted: &mut mpsc::UnboundedReceiver<String>,
    ) -> Result<Tokens, Error> {
        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, async {
            tokio::select! {
                code = self.redirected() => code,
                code = self.pasted(pasted) => code,
            }
        })
        .await
        .map_err(|_| Error::TimedOut)??;
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", self.redirect.as_str()),
            ("client_id", self.client_id.as_str()),
            ("code_verifier", self.verifier.as_str()),
        ];
        if let Some(secret) = client_secret {
            form.push(("client_secret", secret));
        }
        let tokens = token_request(&self.token_url, &form).await?;
        if tokens.refresh_token.is_none() {
            return Err(Error::NoRefreshToken);
        }
        Ok(tokens)
    }

    /// The code the browser comes back to the loopback port with; requests for anything else (a favicon) are
    /// turned away.
    async fn redirected(&self) -> Result<String, Error> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|error| Error::Redirect(error.to_string()))?;
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n")
                && request.len() < 64 * 1024
            {
                let read = stream
                    .read(&mut buffer)
                    .await
                    .map_err(|error| Error::Redirect(error.to_string()))?;
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8_lossy(&request);
            let target = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");
            let Some(answer) = Url::parse(&format!("http://127.0.0.1{target}"))
                .ok()
                .and_then(|url| self.answer(&url))
            else {
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                continue;
            };
            let (status, said) = match answer {
                Ok(_) => ("200 OK", "muttrs is signed in. You can close this tab."),
                Err(_) => ("400 Bad Request", "Signing in didn't work: see muttrs."),
            };
            let page = format!(
                "<!doctype html><meta charset=utf-8><title>muttrs</title><p style=\"font: 16px sans-serif\">{said}</p>"
            );
            let _ = stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n{page}",
                        page.len()
                    )
                    .as_bytes(),
                )
                .await;
            return answer;
        }
    }

    /// The code in the next address `pasted` gives that isn't blank; never, once `pasted` closes.
    async fn pasted(&self, pasted: &mut mpsc::UnboundedReceiver<String>) -> Result<String, Error> {
        loop {
            let Some(address) = pasted.recv().await else {
                return std::future::pending().await;
            };
            let address = address.trim();
            if address.is_empty() {
                continue;
            }
            // Browsers may leave out the scheme of what they show.
            return Url::parse(address)
                .or_else(|_| Url::parse(&format!("http://{address}")))
                .ok()
                .and_then(|url| self.answer(&url))
                .unwrap_or(Err(Error::NotTheRedirect));
        }
    }

    /// What the browser came back with, at `url`: the code, or why the provider refused; None if `url` isn't this
    /// sign-in's answer.
    fn answer(&self, url: &Url) -> Option<Result<String, Error>> {
        let query = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if query("state").as_deref() != Some(self.state.as_str()) {
            return None;
        }
        Some(match (query("code"), query("error")) {
            (Some(code), _) => Ok(code),
            (None, Some(error)) => Err(Error::Refused(error)),
            (None, None) => Err(Error::Redirect("no code in the redirect".into())),
        })
    }
}

/// A fresh access token, from `refresh_token`.
///
/// # Errors
///
/// If the provider can't be reached, or refuses (the refresh token was revoked, say).
pub async fn refresh(
    oauth2: &OAuth2,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<Tokens, Error> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", oauth2.client_id.as_str()),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    token_request(&oauth2.token_url, &form).await
}

async fn token_request(url: &str, form: &[(&str, &str)]) -> Result<Tokens, Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response: TokenResponse = client.post(url).form(form).send().await?.json().await?;
    match response {
        TokenResponse {
            access_token: Some(access_token),
            expires_in,
            refresh_token,
            ..
        } => Ok(Tokens {
            access_token,
            expires: SystemTime::now() + Duration::from_secs(expires_in.unwrap_or(3600)),
            refresh_token,
        }),
        TokenResponse {
            error,
            error_description,
            ..
        } => Err(Error::Refused(
            [error, error_description]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(": "),
        )),
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpStream;

    use super::*;

    #[test]
    fn pkce_challenges_are_rfc_7636s() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(random(32).len(), 43);
    }

    /// The query parameter `name` of `url`.
    fn parameter(url: &str, name: &str) -> String {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap()
    }

    /// Plays the browser: follows the authorization URL's redirect with `code`, after asking for a favicon.
    async fn browse(authorization: &str, code: &str) -> String {
        let redirect = Url::parse(&parameter(authorization, "redirect_uri")).unwrap();
        let address = format!("127.0.0.1:{}", redirect.port().unwrap());
        let mut favicon = TcpStream::connect(&address).await.unwrap();
        favicon
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut ignored = String::new();
        favicon.read_to_string(&mut ignored).await.unwrap();
        assert!(ignored.starts_with("HTTP/1.1 404"));

        let mut stream = TcpStream::connect(&address).await.unwrap();
        let state = parameter(authorization, "state");
        stream
            .write_all(
                format!("GET /?state={state}&code={code}&scope=x HTTP/1.1\r\nHost: x\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut page = String::new();
        stream.read_to_string(&mut page).await.unwrap();
        page
    }

    /// A token endpoint that gives tokens for the code `the-code`, and a client of it.
    async fn provider() -> (wiremock::MockServer, OAuth2) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/token"))
            .and(wiremock::matchers::body_string_contains("grant_type=authorization_code"))
            .and(wiremock::matchers::body_string_contains("code=the-code"))
            .and(wiremock::matchers::body_string_contains("code_verifier="))
            .and(wiremock::matchers::body_string_contains("client_secret=shh"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access", "expires_in": 3599, "refresh_token": "refresh", "token_type": "Bearer"
            })))
            .mount(&server)
            .await;
        let oauth2 = OAuth2 {
            client_id: "client".into(),
            client_secret: None,
            auth_url: "https://accounts.example/auth".into(),
            token_url: format!("{}/token", server.uri()),
            scopes: vec!["https://mail.google.com/".into()],
        };
        (server, oauth2)
    }

    #[tokio::test]
    async fn signing_in_through_the_browser() {
        let (_server, oauth2) = provider().await;
        let authorization = Authorization::start(&oauth2, "ann@example.com").unwrap();
        let url = authorization.url();
        assert!(url.starts_with("https://accounts.example/auth?response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("login_hint=ann%40example.com"));
        // Nothing's pasted, but nothing stops it being.
        let (_paste, mut pasted) = mpsc::unbounded_channel();
        let (tokens, page) = tokio::join!(
            authorization.tokens(Some("shh"), &mut pasted),
            browse(url, "the-code"),
        );
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");

        let tokens = tokens.unwrap();
        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
        assert!(tokens.expires > SystemTime::now() + Duration::from_secs(3500));
    }

    #[tokio::test]
    async fn pasting_the_address_the_browser_ended_at() {
        let (_server, oauth2) = provider().await;
        let authorization = Authorization::start(&oauth2, "ann@example.com").unwrap();
        let (redirect, state) = (
            parameter(authorization.url(), "redirect_uri"),
            parameter(authorization.url(), "state"),
        );
        let (paste, mut pasted) = mpsc::unbounded_channel();

        for wrong in [
            "the-code".to_owned(),
            format!("{redirect}/?state=another&code=the-code"),
        ] {
            paste.send(wrong).unwrap();
            assert!(matches!(
                authorization.tokens(Some("shh"), &mut pasted).await,
                Err(Error::NotTheRedirect)
            ));
        }

        paste.send(" \n".into()).unwrap();
        // As a browser may show it, without its scheme.
        let shown = redirect.trim_start_matches("http://");
        paste
            .send(format!(" {shown}/?state={state}&code=the-code&scope=x\n"))
            .unwrap();
        let tokens = authorization
            .tokens(Some("shh"), &mut pasted)
            .await
            .unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
    }

    #[tokio::test]
    async fn refusing_in_the_browser() {
        let (_server, oauth2) = provider().await;
        let authorization = Authorization::start(&oauth2, "ann@example.com").unwrap();
        let (redirect, state) = (
            parameter(authorization.url(), "redirect_uri"),
            parameter(authorization.url(), "state"),
        );
        let (paste, mut pasted) = mpsc::unbounded_channel();
        paste
            .send(format!("{redirect}/?state={state}&error=access_denied"))
            .unwrap();
        let error = authorization
            .tokens(Some("shh"), &mut pasted)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "the provider refused: access_denied");
    }

    #[tokio::test]
    async fn refusals_say_why() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant", "error_description": "Token has been expired or revoked."
            })))
            .mount(&server)
            .await;
        let oauth2 = OAuth2 {
            client_id: "client".into(),
            client_secret: None,
            auth_url: String::new(),
            token_url: server.uri(),
            scopes: Vec::new(),
        };
        let error = refresh(&oauth2, None, "old").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "the provider refused: invalid_grant: Token has been expired or revoked."
        );
    }
}
