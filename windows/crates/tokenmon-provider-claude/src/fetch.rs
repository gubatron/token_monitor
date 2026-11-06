//! HTTP glue: sends the claude.ai usage request through core's
//! [`ProviderHttp`] and parses the body.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokenmon_core::BoxFuture;
use tokenmon_core::net::{ProviderHttp, RequestOptions};
use tokenmon_core::{Clock, ProviderError};

use crate::usage::{ClaudeUsageResponse, parse_usage_body, usage_request};

/// Production origin of the usage endpoint.
pub const DEFAULT_BASE_URL: &str = "https://claude.ai";

/// One completed fetch: the parsed payload and when it was taken.
pub type FetchedUsage = (ClaudeUsageResponse, DateTime<Utc>);

/// Fetches claude.ai rate-limit usage for a cookie header.
#[derive(Debug, Clone)]
pub struct ClaudeUsageClient {
    http: ProviderHttp,
    base_url: String,
}

impl ClaudeUsageClient {
    /// A client for `base_url` (use [`DEFAULT_BASE_URL`]; tests point it at a
    /// mock server). `http` must carry the Claude error context.
    pub fn new(http: ProviderHttp, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
        }
    }

    /// Fetches usage for the organization in `cookie_header`.
    ///
    /// # Errors
    ///
    /// * `NotSignedIn` (custom) when the cookie header has no org id; no
    ///   request is made.
    /// * Transport, status and challenge errors from [`ProviderHttp`].
    /// * [`parse_usage_body`] errors for a 2xx body that is not usage JSON (a
    ///   login page is `Unauthorized`).
    pub async fn fetch_usage(
        &self,
        cookie_header: &str,
        now: DateTime<Utc>,
    ) -> Result<FetchedUsage, ProviderError> {
        let request = usage_request(cookie_header)?;
        let body = self
            .http
            .get(
                &self.base_url,
                &request.path,
                RequestOptions {
                    cookie_header: Some(&request.cookie_header),
                    referer: Some(request.referer),
                    ..RequestOptions::default()
                },
            )
            .await?;
        Ok((parse_usage_body(&body)?, now))
    }

    /// The fetch seam [`crate::poller::ClaudeUsagePoller`] takes, stamping
    /// each result with `clock.now()`.
    pub fn into_fetch_fn(self, clock: Arc<dyn Clock>) -> Arc<crate::poller::FetchUsageFn> {
        let client = Arc::new(self);
        Arc::new(
            move |cookie: String| -> BoxFuture<'static, Result<FetchedUsage, ProviderError>> {
                let client = Arc::clone(&client);
                let now = clock.now();
                Box::pin(async move { client.fetch_usage(&cookie, now).await })
            },
        )
    }
}
