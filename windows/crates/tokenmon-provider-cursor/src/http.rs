//! HTTP plumbing shared by Cursor and Grokbot (`ProviderHTTP`, spec 02
//! sections 2.2, 3.2).
//!
//! Both providers call `cursor.com` with the same cookie header and the same
//! status mapping. They differ only in the `Referer`, the error context and in
//! what a non-challenge 403 means ([`ForbiddenPolicy`]). This module is a
//! stand-in for the ws-core-net client wrapper: when that lands, the transport
//! half can be swapped without touching callers, because the contract is only
//! "bytes in, `ProviderError` out".
//!
//! Secrets: the cookie header is passed per call and never stored, logged or
//! included in any error text.

use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, USER_AGENT};
use serde_json::{Map, Value};
use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};

/// Production base URL.
pub const CURSOR_BASE_URL: &str = "https://cursor.com";

/// Upper bound on a response body we are willing to buffer.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// How many leading bytes are scanned for login and challenge markers.
const MARKER_SCAN_BYTES: usize = 64 * 1024;

const LOGIN_MARKERS: [&str; 10] = [
    "type=\"password\"",
    "type='password'",
    "sign in",
    "sign-in",
    "signin",
    "log in",
    "log-in",
    "login",
    "authkit",
    "workos",
];

const CHALLENGE_MARKERS: [&str; 6] = [
    "cf-chl",
    "challenge-platform",
    "cf_chl_opt",
    "just a moment...",
    "attention required! | cloudflare",
    "cf-browser-verification",
];

/// What a 403 that is not a bot challenge means for a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenPolicy {
    /// Treated as an expired session ([`ProviderError::Unauthorized`]); Cursor.
    Unauthorized,
    /// A transient, pre-worded error that is not an auth strike; Grokbot.
    Custom(&'static str),
}

/// Per-call error wording and 403 handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPolicy {
    /// Provider used for error wording.
    pub context: ProviderErrorContext,
    /// Meaning of a non-challenge 403.
    pub forbidden: ForbiddenPolicy,
}

impl RequestPolicy {
    /// Cursor: a 403 is an expired session.
    pub const CURSOR: Self = Self {
        context: ProviderErrorContext::Cursor,
        forbidden: ForbiddenPolicy::Unauthorized,
    };
    /// Grokbot: a 403 means the account has no Grok Bot access.
    pub const GROKBOT: Self = Self {
        context: ProviderErrorContext::Grokbot,
        forbidden: ForbiddenPolicy::Custom("Grok Bot is not available on this Cursor account."),
    };
}

/// A thin `reqwest` client bound to a base URL and a user agent.
///
/// Cheap to clone (the inner client is reference counted).
#[derive(Debug, Clone)]
pub struct CursorHttp {
    client: reqwest::Client,
    base_url: String,
    user_agent: String,
}

impl CursorHttp {
    /// Client for `base_url` (no trailing slash needed) with the platform
    /// default 60 s timeout. `user_agent` is `TokenMon/<version>`.
    ///
    /// # Panics
    /// If the TLS backend cannot be initialised, which is a build defect.
    pub fn new(base_url: &str, user_agent: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client with default TLS configuration");
        Self::with_client(client, base_url, user_agent)
    }

    /// Uses a caller-provided `reqwest` client (for shared pools or tests).
    pub fn with_client(client: reqwest::Client, base_url: &str, user_agent: &str) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            user_agent: user_agent.to_owned(),
        }
    }

    /// `GET <base><path>` with the shared headers; returns the 2xx body.
    pub async fn get(
        &self,
        path: &str,
        cookie: &str,
        referer: &str,
        policy: RequestPolicy,
    ) -> Result<Vec<u8>, ProviderError> {
        let request = self.base_request(self.client.get(self.url(path)), cookie, referer);
        send(request, policy).await
    }

    /// `POST <base><path>` with a JSON body, `Origin` and `Content-Type`;
    /// returns the 2xx body.
    pub async fn post_json(
        &self,
        path: &str,
        cookie: &str,
        referer: &str,
        body: &Value,
        policy: RequestPolicy,
    ) -> Result<Vec<u8>, ProviderError> {
        let request = self
            .base_request(self.client.post(self.url(path)), cookie, referer)
            .header(ORIGIN, CURSOR_BASE_URL)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string());
        send(request, policy).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn base_request(
        &self,
        request: reqwest::RequestBuilder,
        cookie: &str,
        referer: &str,
    ) -> reqwest::RequestBuilder {
        request
            .header(COOKIE, cookie)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, &self.user_agent)
            .header(REFERER, referer)
    }
}

async fn send(
    request: reqwest::RequestBuilder,
    policy: RequestPolicy,
) -> Result<Vec<u8>, ProviderError> {
    let ctx = policy.context;
    let mut response = request
        .send()
        .await
        .map_err(|e| ProviderError::Network(ctx, e.without_url().to_string()))?;
    let status = response.status().as_u16();
    let challenged = response.headers().contains_key("cf-mitigated");
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_retry_after);
    let body = read_capped(&mut response, ctx).await?;
    map_status(status, challenged, retry_after, &body, policy)?;
    Ok(body)
}

async fn read_capped(
    response: &mut reqwest::Response,
    ctx: ProviderErrorContext,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ProviderError::Network(ctx, e.without_url().to_string()))?
    {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ProviderError::BadResponse(
                ctx,
                "Response too large".to_owned(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `Retry-After` in whole or fractional seconds; `None` when not a sane number.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::parse_retry_after;
/// assert_eq!(parse_retry_after(" 30 "), Some(30.0));
/// assert_eq!(parse_retry_after("-1"), None);
/// assert_eq!(parse_retry_after("soon"), None);
/// ```
pub fn parse_retry_after(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
}

/// Maps an HTTP status (plus challenge/Retry-After hints) to the shared
/// error taxonomy. `Ok(())` for 2xx.
///
/// Order matters: a bot challenge wins over every status code, then 401, 403,
/// 429, other non-2xx.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::{map_status, RequestPolicy};
/// assert!(map_status(200, false, None, b"{}", RequestPolicy::CURSOR).is_ok());
/// assert!(map_status(401, false, None, b"", RequestPolicy::CURSOR).is_err());
/// ```
pub fn map_status(
    status: u16,
    challenged: bool,
    retry_after: Option<f64>,
    body: &[u8],
    policy: RequestPolicy,
) -> Result<(), ProviderError> {
    let ctx = policy.context;
    if challenged || (status == 403 && looks_like_html(body)) {
        return Err(ProviderError::BadResponse(
            ctx,
            format!("Request blocked by a bot-protection check (HTTP {status}). Retrying."),
        ));
    }
    match status {
        200..=299 => Ok(()),
        401 => Err(ProviderError::Unauthorized(ctx)),
        403 => Err(match policy.forbidden {
            ForbiddenPolicy::Unauthorized => ProviderError::Unauthorized(ctx),
            ForbiddenPolicy::Custom(message) => ProviderError::Custom {
                message: message.to_owned(),
                usage: UsageError::BadResponse(message.to_owned()),
            },
        }),
        429 => Err(ProviderError::Custom {
            message: format!(
                "{} is rate limiting requests. Retrying later.",
                ctx.display_name()
            ),
            usage: UsageError::RateLimited { retry_after },
        }),
        other => Err(ProviderError::BadResponse(ctx, format!("HTTP {other}"))),
    }
}

/// Whether the trimmed body starts with `<`.
pub fn looks_like_html(body: &[u8]) -> bool {
    body.iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'<')
}

fn scan_prefix(body: &[u8]) -> Vec<u8> {
    body[..body.len().min(MARKER_SCAN_BYTES)].to_ascii_lowercase()
}

fn contains_any(haystack: &[u8], needles: &[&str]) -> bool {
    needles.iter().any(|n| {
        let n = n.as_bytes();
        haystack.windows(n.len()).any(|w| w == n)
    })
}

/// Whether an HTML body is a sign-in page: a login marker and no bot-challenge
/// marker in the lowercased first 64 KiB (bytes are compared as Latin-1, so
/// non-UTF-8 input is handled).
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::looks_like_login_page;
/// assert!(looks_like_login_page(b"<html>Sign in to Cursor</html>"));
/// assert!(!looks_like_login_page(b"<html>Just a moment... login</html>"));
/// ```
pub fn looks_like_login_page(body: &[u8]) -> bool {
    let prefix = scan_prefix(body);
    contains_any(&prefix, &LOGIN_MARKERS) && !contains_any(&prefix, &CHALLENGE_MARKERS)
}

/// `ProviderHTTP.jsonObject`: the body as a JSON object, or the right error.
///
/// * HTML that is a login page gives `Unauthorized`.
/// * Other HTML gives `BadResponse("Unexpected HTML response")`.
/// * Anything else that is not a JSON object gives
///   `BadResponse("Malformed response body")`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::ProviderErrorContext;
/// use tokenmon_provider_cursor::json_object;
/// let ctx = ProviderErrorContext::Cursor;
/// assert!(json_object(b"{\"a\":1}", ctx).is_ok());
/// assert!(json_object(b"[1]", ctx).is_err());
/// ```
pub fn json_object(
    body: &[u8],
    ctx: ProviderErrorContext,
) -> Result<Map<String, Value>, ProviderError> {
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
        return Ok(map);
    }
    if looks_like_html(body) {
        return Err(if looks_like_login_page(body) {
            ProviderError::Unauthorized(ctx)
        } else {
            ProviderError::BadResponse(ctx, "Unexpected HTML response".to_owned())
        });
    }
    Err(ProviderError::BadResponse(
        ctx,
        "Malformed response body".to_owned(),
    ))
}

/// `rejectUnauthorizedBody`: [`json_object`], then an `error` string that
/// contains `not_authenticated` or `unauthor` becomes `Unauthorized`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::{ProviderError, ProviderErrorContext};
/// use tokenmon_provider_cursor::reject_unauthorized_body;
/// let e = reject_unauthorized_body(b"{\"error\":\"not_authenticated\"}", ProviderErrorContext::Cursor);
/// assert!(matches!(e, Err(ProviderError::Unauthorized(_))));
/// ```
pub fn reject_unauthorized_body(
    body: &[u8],
    ctx: ProviderErrorContext,
) -> Result<Map<String, Value>, ProviderError> {
    let map = json_object(body, ctx)?;
    if let Some(Value::String(error)) = map.get("error") {
        let lower = error.to_lowercase();
        if lower.contains("not_authenticated") || lower.contains("unauthor") {
            return Err(ProviderError::Unauthorized(ctx));
        }
    }
    Ok(map)
}
