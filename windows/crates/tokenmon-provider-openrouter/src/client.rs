//! HTTP access and the `/key` then `/credits` + `/activity` orchestration
//! (`OpenRouterUsageClient`, spec 03 section 4.2).
//!
//! Transport, status mapping, bot-challenge detection and `Retry-After` come
//! from `tokenmon_core::net::ProviderHttp`; [`EndpointFetcher`] is the seam
//! tests fake.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokenmon_core::net::{ProviderHttp, RequestOptions};
use tokenmon_core::{BoxFuture, Clock, ProviderError, ProviderErrorContext};

use crate::activity::CostEstimator;
use crate::auth::ApiKey;
use crate::models::{parse_activity, parse_credits, parse_key};
use crate::snapshot::OpenRouterSnapshot;

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
    pub fn url(self, base: &str) -> Option<url::Url> {
        ProviderHttp::resolve(self.path(), base)
    }
}

/// Fetches one endpoint's body.
pub trait EndpointFetcher: Send + Sync {
    /// GET the endpoint with the key's bearer auth.
    fn fetch(&self, endpoint: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>>;
}

/// Live transport over the core [`ProviderHttp`].
pub struct ReqwestFetcher {
    http: ProviderHttp,
    base: String,
    key: ApiKey,
}

impl fmt::Debug for ReqwestFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestFetcher")
            .field("base", &self.base)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl ReqwestFetcher {
    /// A fetcher for `base` (use [`DEFAULT_BASE_URL`] in production).
    ///
    /// # Errors
    /// [`ProviderError::Network`] if the HTTP client cannot be built.
    pub fn new(
        base: &str,
        key: ApiKey,
        app_version: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProviderError> {
        let http = ProviderHttp::new(ProviderErrorContext::OpenRouter, app_version, clock)?;
        Ok(Self {
            http,
            base: base.to_owned(),
            key,
        })
    }
}

impl EndpointFetcher for ReqwestFetcher {
    fn fetch(&self, endpoint: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>> {
        Box::pin(async move {
            let options = RequestOptions {
                bearer_token: Some(self.key.expose()),
                ..RequestOptions::default()
            };
            self.http
                .get(&self.base, endpoint.path(), options)
                .await
                .map_err(|e| redact_error(&self.key, e))
        })
    }
}

/// Defense in depth: scrubs the key from every string an error carries.
fn redact_error(key: &ApiKey, e: ProviderError) -> ProviderError {
    match e {
        ProviderError::Network(c, m) => ProviderError::Network(c, key.redact(&m)),
        ProviderError::BadResponse(c, m) => ProviderError::BadResponse(c, key.redact(&m)),
        ProviderError::Custom { message, usage } => ProviderError::Custom {
            message: key.redact(&message),
            usage,
        },
        other => other,
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
