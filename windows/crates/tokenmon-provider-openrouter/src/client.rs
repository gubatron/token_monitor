//! HTTP access and the `/key` then `/credits` + `/activity` orchestration
//! (`OpenRouterUsageClient`, spec 03 section 4.2).
//!
//! [`ReqwestFetcher`] is a deliberately small private transport (status
//! mapping per the spec's shared rules). It is replaced by the `tokenmon-core`
//! net wrapper once that lands; the [`EndpointFetcher`] seam stays.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::{BoxFuture, ProviderError, ProviderErrorContext, UsageError};
use url::Url;

use crate::activity::CostEstimator;
use crate::auth::ApiKey;
use crate::models::{parse_activity, parse_credits, parse_key};
use crate::snapshot::OpenRouterSnapshot;

const CTX: ProviderErrorContext = ProviderErrorContext::OpenRouter;

/// Production API base.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// The three read endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// `/key`, any key.
    Key,
    /// `/credits`, management keys.
    Credits,
    /// `/activity`, management keys.
    Activity,
}

impl Endpoint {
    /// All endpoints.
    pub const ALL: [Self; 3] = [Self::Key, Self::Credits, Self::Activity];

    /// Path relative to the API base.
    pub const fn path(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Credits => "credits",
            Self::Activity => "activity",
        }
    }

    /// Absolute URL under `base`, treating a base without a trailing slash as
    /// a directory (`.../api/v1` + `key` is `.../api/v1/key`).
    pub fn url(self, base: &Url) -> Option<Url> {
        let mut dir = base.clone();
        if !dir.path().ends_with('/') {
            dir.set_path(&format!("{}/", dir.path()));
        }
        dir.join(self.path()).ok()
    }
}

/// Fetches one endpoint's body.
pub trait EndpointFetcher: Send + Sync {
    /// GET the endpoint with the key's bearer auth.
    fn fetch(&self, endpoint: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>>;
}

/// Live reqwest transport.
pub struct ReqwestFetcher {
    client: reqwest::Client,
    base: Url,
    key: ApiKey,
}

impl fmt::Debug for ReqwestFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestFetcher")
            .field("base", &self.base.as_str())
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl ReqwestFetcher {
    /// A fetcher for `base` (use [`DEFAULT_BASE_URL`] in production).
    ///
    /// # Errors
    /// [`ProviderError::Network`] if the HTTP client cannot be built.
    pub fn new(base: &str, key: ApiKey) -> Result<Self, ProviderError> {
        let base = Url::parse(base)
            .map_err(|_| ProviderError::BadResponse(CTX, "Invalid base URL".into()))?;
        let client = reqwest::Client::builder()
            .user_agent(concat!("TokenMon/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ProviderError::Network(CTX, key.redact(&e.without_url().to_string())))?;
        Ok(Self { client, base, key })
    }

    async fn get(&self, endpoint: Endpoint) -> Result<Vec<u8>, ProviderError> {
        let url = endpoint
            .url(&self.base)
            .ok_or_else(|| ProviderError::BadResponse(CTX, "Invalid endpoint URL".into()))?;
        let response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .bearer_auth(self.key.expose())
            .send()
            .await
            .map_err(|e| self.network_error(&e))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<f64>().ok());
        let bot_challenge = response.headers().contains_key("cf-mitigated");
        let body = response.bytes().await.map_err(|e| self.network_error(&e))?;
        map_status(status.as_u16(), retry_after, bot_challenge, &body).map(|()| body.to_vec())
    }

    fn network_error(&self, e: &reqwest::Error) -> ProviderError {
        ProviderError::Network(CTX, self.key.redact(&e.to_string()))
    }
}

/// Shared status mapping (spec 03 conventions).
fn map_status(
    status: u16,
    retry_after: Option<f64>,
    bot_challenge: bool,
    body: &[u8],
) -> Result<(), ProviderError> {
    let html = body
        .iter()
        .take(512)
        .copied()
        .collect::<Vec<_>>()
        .to_ascii_lowercase()
        .windows(5)
        .any(|w| w == b"<html" || w == b"<!doc");
    if bot_challenge || (status == 403 && html) {
        return Err(ProviderError::BadResponse(
            CTX,
            format!("Request blocked by a bot-protection check (HTTP {status}). Retrying."),
        ));
    }
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(ProviderError::Unauthorized(CTX)),
        429 => Err(ProviderError::Custom {
            message: "OpenRouter is rate limiting requests. Retrying later.".into(),
            usage: UsageError::RateLimited { retry_after },
        }),
        s => Err(ProviderError::BadResponse(CTX, format!("HTTP {s}"))),
    }
}

impl EndpointFetcher for ReqwestFetcher {
    fn fetch(&self, endpoint: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>> {
        Box::pin(self.get(endpoint))
    }
}

/// Orchestrates the three endpoints into a snapshot.
pub struct OpenRouterUsageClient {
    fetcher: Arc<dyn EndpointFetcher>,
    estimator: Arc<dyn CostEstimator>,
}

impl fmt::Debug for OpenRouterUsageClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenRouterUsageClient")
            .finish_non_exhaustive()
    }
}

impl OpenRouterUsageClient {
    /// A client over any transport.
    pub fn new(fetcher: Arc<dyn EndpointFetcher>, estimator: Arc<dyn CostEstimator>) -> Self {
        Self { fetcher, estimator }
    }

    /// Fetches `/key`; when the key may be a management key (flag true or
    /// absent) also `/credits` and `/activity` concurrently. Failures of those
    /// two degrade quietly to `None`.
    ///
    /// # Errors
    /// Failures of `/key`: HTTP errors propagate (401 and 403 count as auth
    /// strikes), a malformed body is [`ProviderError::BadResponse`].
    pub async fn fetch_snapshot(
        &self,
        now: DateTime<Utc>,
    ) -> Result<OpenRouterSnapshot, ProviderError> {
        let key = parse_key(&self.fetcher.fetch(Endpoint::Key).await?)?;
        if key.is_management_key == Some(false) {
            return Ok(self.build(&key, None, None, now));
        }
        let (credits, activity) = futures::join!(
            self.fetcher.fetch(Endpoint::Credits),
            self.fetcher.fetch(Endpoint::Activity)
        );
        let credits = credits.ok().and_then(|b| parse_credits(&b).ok());
        let activity = activity.ok().and_then(|b| parse_activity(&b).ok());
        Ok(self.build(&key, credits, activity.as_deref(), now))
    }

    fn build(
        &self,
        key: &crate::models::KeyData,
        credits: Option<crate::models::CreditsData>,
        activity: Option<&[crate::models::ActivityRow]>,
        now: DateTime<Utc>,
    ) -> OpenRouterSnapshot {
        OpenRouterSnapshot::build(key, credits, activity, now, self.estimator.as_ref())
    }
}
