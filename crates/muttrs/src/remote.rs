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
