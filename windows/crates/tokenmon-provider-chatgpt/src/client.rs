//! Session exchange and usage fetch (spec 03 section 2.2).

use std::future::Future;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokenmon_core::UsageError;
use tokenmon_core::net::{
    ProviderError, ProviderErrorContext, ProviderHttp, RequestOptions, looks_like_html,
};
use tokenmon_core::util::json::string;

use crate::jwt::account_id;
use crate::token_cache::{AccessTokenCache, valid_until};
use crate::usage::{UsageResponse, parse_usage};

/// Production base URL.
pub const BASE_URL: &str = "https://chatgpt.com";
const REFERER: &str = "https://chatgpt.com/";
const CTX: ProviderErrorContext = ProviderErrorContext::ChatGpt;

/// One usage refresh: the parsed payload plus every `Set-Cookie` returned
/// (NextAuth renews the session cookie on `/api/auth/session`).
#[derive(Debug, Clone, PartialEq)]
pub struct Fetch {
    /// Parsed usage.
    pub response: UsageResponse,
    /// The `now` the fetch ran at.
    pub fetched_at: DateTime<Utc>,
    /// Session cookies first, then usage cookies.
    pub set_cookie_headers: Vec<String>,
}

/// A body and the `Set-Cookie` values that came with it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawReply {
    /// Response body.
    pub body: Vec<u8>,
    /// Each `Set-Cookie` value.
    pub set_cookies: Vec<String>,
}

/// Network seam (Swift `Transport`): the two requests, so tests inject fakes.
pub trait ChatGptTransport: Send + Sync {
    /// `GET /api/auth/session` with the cookie header.
    fn session(
        &self,
        cookie_header: &str,
    ) -> impl Future<Output = Result<RawReply, ProviderError>> + Send;

    /// `GET /backend-api/wham/usage` with bearer token and optional account id.
    fn usage(
        &self,
        cookie_header: &str,
        access_token: &str,
        account_id: Option<&str>,
    ) -> impl Future<Output = Result<RawReply, ProviderError>> + Send;
}

/// Live transport over [`ProviderHttp`].
#[derive(Debug, Clone)]
pub struct HttpTransport {
    http: ProviderHttp,
    base_url: String,
}

impl HttpTransport {
    /// Transport against [`BASE_URL`].
    pub fn new(http: ProviderHttp) -> Self {
        Self::with_base_url(http, BASE_URL)
    }

    /// Transport against another origin (tests point this at wiremock).
    pub fn with_base_url(http: ProviderHttp, base_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.to_owned(),
        }
    }
}

impl ChatGptTransport for HttpTransport {
    async fn session(&self, cookie_header: &str) -> Result<RawReply, ProviderError> {
        let options = RequestOptions {
            cookie_header: Some(cookie_header),
            referer: Some(REFERER),
            ..RequestOptions::default()
        };
        let r = self
            .http
            .get_with_response(&self.base_url, "api/auth/session", options)
            .await?;
        Ok(RawReply {
            set_cookies: r.set_cookie_headers(),
            body: r.body,
        })
    }

    async fn usage(
        &self,
        cookie_header: &str,
        access_token: &str,
        account_id: Option<&str>,
    ) -> Result<RawReply, ProviderError> {
        let extra: Vec<(&str, &str)> = account_id
            .map(|id| ("ChatGPT-Account-Id", id))
            .into_iter()
            .collect();
        let options = RequestOptions {
            cookie_header: Some(cookie_header),
            bearer_token: Some(access_token),
            referer: Some(REFERER),
            headers: &extra,
            ..RequestOptions::default()
        };
        let r = self
            .http
            .get_with_response(&self.base_url, "/backend-api/wham/usage", options)
            .await?;
        Ok(RawReply {
            set_cookies: r.set_cookie_headers(),
            body: r.body,
        })
    }
}

/// Parses the `/api/auth/session` payload for the bearer token.
///
/// # Errors
///
/// A non-JSON-object body is a transient `BadResponse` (the credential is kept:
/// such a body also appears while the session is valid); a missing, non-string
/// or empty `accessToken` is `Unauthorized`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::parse_session_token;
/// assert_eq!(parse_session_token(br#"{"accessToken":"tok-123"}"#).unwrap(), "tok-123");
/// assert!(parse_session_token(b"{}").is_err());
/// ```
pub fn parse_session_token(data: &[u8]) -> Result<String, ProviderError> {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(data) else {
        let message = if looks_like_html(data) {
            "Sign-in page returned instead of a session token"
        } else {
            "Malformed session payload"
        };
        return Err(ProviderError::BadResponse(CTX, message.into()));
    };
    match string(root.get("accessToken")) {
        Some(token) if !token.is_empty() => Ok(token.to_owned()),
        _ => Err(ProviderError::Unauthorized(CTX)),
    }
}

/// Fetches Codex usage for one session cookie, reusing a cached access token.
pub struct ChatGptUsageClient<T: ChatGptTransport> {
    cookie_header: String,
    transport: Arc<T>,
    token_cache: Option<Arc<AccessTokenCache>>,
    token_cache_key: String,
}

impl<T: ChatGptTransport> ChatGptUsageClient<T> {
    /// `token_cache = None` exchanges the cookie on every call. `token_cache_key`
    /// identifies the signed-in session (stable while the cookie renews).
    pub fn new(
        cookie_header: impl Into<String>,
        transport: Arc<T>,
        token_cache: Option<Arc<AccessTokenCache>>,
        token_cache_key: impl Into<String>,
    ) -> Self {
        Self {
            cookie_header: cookie_header.into(),
            transport,
            token_cache,
            token_cache_key: token_cache_key.into(),
        }
    }

    /// Fetches usage with a cached token when valid, else exchanges the cookie.
    /// A cached token rejected with `unauthorized` is dropped and the call is
    /// retried once with a fresh exchange; any other error propagates.
    ///
    /// # Errors
    ///
    /// Transport, status and parse errors as [`ProviderError`].
    pub async fn fetch_usage(&self, now: DateTime<Utc>) -> Result<Fetch, ProviderError> {
        if let Some(cached) = self.cached_token(now) {
            match self.usage(&cached, Vec::new(), now).await {
                Err(e) if e.usage_error() == UsageError::Unauthorized => self.clear_cache(),
                other => return other,
            }
        }
        let reply = self.transport.session(&self.cookie_header).await?;
        let token = parse_session_token(&reply.body)?;
        if let Some(cache) = &self.token_cache {
            cache.store(&token, &self.token_cache_key, valid_until(&token, now));
        }
        self.usage(&token, reply.set_cookies, now).await
    }

    fn cached_token(&self, now: DateTime<Utc>) -> Option<String> {
        self.token_cache.as_ref()?.token(&self.token_cache_key, now)
    }

    fn clear_cache(&self) {
        if let Some(cache) = &self.token_cache {
            cache.clear();
        }
    }

    async fn usage(
        &self,
        access_token: &str,
        mut session_cookies: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<Fetch, ProviderError> {
        let account = account_id(access_token);
        let reply = self
            .transport
            .usage(&self.cookie_header, access_token, account.as_deref())
            .await?;
        let response = parse_usage(&reply.body)?;
        session_cookies.extend(reply.set_cookies);
        Ok(Fetch {
            response,
            fetched_at: now,
            set_cookie_headers: session_cookies,
        })
    }
}
