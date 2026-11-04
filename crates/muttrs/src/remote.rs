//! Remote Content, fetched only when the user asks for it.
//!
//! No cookies, no referrer and a generic agent, so that loading an image tells its sender no more than that the
//! Message was read; images only, and never bigger than [`MAX_BYTES`].

use std::time::Duration;

use anyhow::{Context as _, bail};
use futures_util::StreamExt as _;

/// The most bytes an image may take.
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Fetcher {
    client: reqwest::Client,
}

impl Fetcher {
    /// # Errors
    ///
    /// If the HTTP client can't be set up, as when the platform's certificates can't be read.
    pub(crate) fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0")
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .referer(false)
            .build()
            .context("setting up the HTTP client")?;
        Ok(Self { client })
    }

    /// Unsubscribes from a mailing list by its one-click URL `url`: a POST of `List-Unsubscribe=One-Click` (RFC
    /// 8058).
    ///
    /// # Errors
    ///
    /// If it can't be reached, or says it failed.
    pub(crate) async fn unsubscribe(&self, url: &str) -> anyhow::Result<()> {
        self.client
            .post(url)
            .form(&[("List-Unsubscribe", "One-Click")])
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// The bytes of the image at `url`.
    ///
    /// # Errors
    ///
    /// If it can't be fetched, isn't an image, or is too big.
    pub(crate) async fn image(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let response = self.client.get(url).send().await?.error_for_status()?;
        let kind = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|kind| kind.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Servers often say nothing, or say octet-stream: the bytes tell.
        if !(kind.is_empty()
            || kind.starts_with("image/")
            || kind.starts_with("application/octet-stream"))
        {
            bail!("{url} isn't an image but {kind}");
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            bail!("{url} is too big to show");
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk?);
            if bytes.len() > MAX_BYTES {
                bail!("{url} is too big to show");
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod unsubscribing {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    #[tokio::test]
    async fn one_click_unsubscribing_posts_what_rfc_8058_says() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            // The headers, then the body.
            while !String::from_utf8_lossy(&request).contains("One-Click") {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "the request ended early");
                request.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        let fetcher = Fetcher::new().unwrap();
        fetcher
            .unsubscribe(&format!("http://127.0.0.1:{port}/u/abc?id=1"))
            .await
            .unwrap();
        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(
            request.starts_with("post /u/abc?id=1 http/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("content-type: application/x-www-form-urlencoded"),
            "{request}"
        );
        assert!(
            request.ends_with("\r\n\r\nlist-unsubscribe=one-click"),
            "{request}"
        );
    }
}
