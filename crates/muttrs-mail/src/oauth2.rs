//! OAuth2 for XOAUTH2: the authorization code flow with PKCE, in the browser, redirected to a loopback port
//! (RFC 8252), and access tokens refreshed with the refresh token it gives.

use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::SecureRandom as _;
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

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
}

/// What the token endpoint gives.
#[derive(Debug)]
pub struct Tokens {
    /// The access token, for XOAUTH2.
    pub access_token: String,
    /// When the access token stops working.
    pub expires: Instant,
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

/// Signs in as `login_hint` in the browser, which `open` is asked to open at the provider's page; the tokens the
/// provider then gives.
///
/// # Errors
///
/// If the user doesn't sign in, the redirect never comes, or the provider refuses.
pub async fn authorize(
    oauth2: &OAuth2,
    client_secret: Option<&str>,
    login_hint: &str,
    open: impl FnOnce(&str),
) -> Result<Tokens, Error> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| Error::Redirect(error.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|error| Error::Redirect(error.to_string()))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = random(48);
    let state = random(16);
    let scopes = oauth2.scopes.join(" ");
    let url = reqwest::Url::parse_with_params(
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
    open(url.as_str());

    let code = tokio::time::timeout(SIGN_IN_TIMEOUT, redirected(&listener, &state))
        .await
        .map_err(|_| Error::TimedOut)??;
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("client_id", oauth2.client_id.as_str()),
        ("code_verifier", verifier.as_str()),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }
    let tokens = token_request(&oauth2.token_url, &form).await?;
    if tokens.refresh_token.is_none() {
        return Err(Error::NoRefreshToken);
    }
    Ok(tokens)
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
            expires: Instant::now() + Duration::from_secs(expires_in.unwrap_or(3600)),
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

/// The code the browser is redirected with, once a request for `state` comes; requests for anything else (a
/// favicon) are turned away.
async fn redirected(listener: &TcpListener, state: &str) -> Result<String, Error> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|error| Error::Redirect(error.to_string()))?;
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") && request.len() < 64 * 1024 {
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
        let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
            continue;
        };
        let query = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if query("state").as_deref() != Some(state) {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        }
        let (status, said) = match (query("code"), query("error")) {
            (Some(_), _) => ("200 OK", "muttrs is signed in. You can close this tab."),
            _ => ("400 Bad Request", "Signing in didn't work: see muttrs."),
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
        return match (query("code"), query("error")) {
            (Some(code), _) => Ok(code),
            (None, Some(error)) => Err(Error::Refused(error)),
            (None, None) => Err(Error::Redirect("no code in the redirect".into())),
        };
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

    /// Plays the browser: follows the authorization URL's redirect with `code`, after asking for a favicon.
    async fn browse(authorization: String, code: &str) -> String {
        let url = reqwest::Url::parse(&authorization).unwrap();
        let pair = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .unwrap()
        };
        let redirect = reqwest::Url::parse(&pair("redirect_uri")).unwrap();
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
        let state = pair("state");
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

    #[tokio::test]
    async fn signing_in_through_the_browser() {
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
        let (opened, mut url) = tokio::sync::mpsc::unbounded_channel();
        let signing_in = tokio::spawn(async move {
            authorize(&oauth2, Some("shh"), "ann@example.com", |url| {
                opened.send(url.to_owned()).unwrap();
            })
            .await
        });
        let authorization = url.recv().await.unwrap();
        assert!(authorization.starts_with("https://accounts.example/auth?response_type=code"));
        assert!(authorization.contains("code_challenge_method=S256"));
        assert!(authorization.contains("login_hint=ann%40example.com"));
        let page = browse(authorization, "the-code").await;
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");

        let tokens = signing_in.await.unwrap().unwrap();
        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh"));
        assert!(tokens.expires > Instant::now() + Duration::from_secs(3500));
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
