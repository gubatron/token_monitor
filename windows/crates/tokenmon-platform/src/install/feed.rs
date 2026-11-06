//! HTTP implementation of [`ReleaseFeedFetcher`].

use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderValue};
use tokenmon_core::BoxFuture;
use tokenmon_core::update::{MAX_FEED_BYTES, ReleaseFeedFetcher, UpdateError};

/// Fetches a GitHub releases endpoint with the headers GitHub requires
/// (notably a `User-Agent`), a 15 s timeout and a body-size cap.
#[derive(Debug, Clone)]
pub struct HttpFeedFetcher {
    client: reqwest::Client,
    url: String,
}

impl HttpFeedFetcher {
    /// Builds a fetcher for `url` (see `ReleaseSource::releases_url`).
    ///
    /// # Errors
    ///
    /// [`UpdateError::Network`] if the HTTP client cannot be built.
    pub fn new(url: impl Into<String>, user_agent: &str) -> Result<Self, UpdateError> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| UpdateError::Network(e.to_string()))?;
        Ok(Self {
            client,
            url: url.into(),
        })
    }

    async fn get(&self) -> Result<Vec<u8>, UpdateError> {
        let mut response = self
            .client
            .get(&self.url)
            .header(
                ACCEPT,
                HeaderValue::from_static("application/vnd.github+json"),
            )
            .header(
                "X-GitHub-Api-Version",
                HeaderValue::from_static("2022-11-28"),
            )
            .send()
            .await
            .map_err(|e| UpdateError::Network(e.without_url().to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(UpdateError::from_status(status.as_u16()));
        }
        let too_large = || UpdateError::BadResponse("Release payload is too large".into());
        if response
            .content_length()
            .is_some_and(|n| n > MAX_FEED_BYTES as u64)
        {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| UpdateError::Network(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > MAX_FEED_BYTES {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

impl ReleaseFeedFetcher for HttpFeedFetcher {
    fn fetch(&self) -> BoxFuture<'_, Result<Vec<u8>, UpdateError>> {
        Box::pin(self.get())
    }
}
