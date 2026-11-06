//! Request headers and the status to [`UsageError`] mapping
//! (spec 01 section 1.1.2, `AuthenticatedRequest`).
//!
//! Everything here is pure so providers and tests can use it without a socket.

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, REFERER, USER_AGENT,
};
use thiserror::Error;

use super::detect::is_bot_challenge;
use super::error::UsageError;
use super::retry_after::retry_after_from_headers;

/// A header could not be built (control characters, newlines...).
///
/// Only the header *name* is reported, never the value: values are secrets.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid value for header {0}")]
pub struct HeaderBuildError(pub String);

/// The `User-Agent` for a given app version: `TokenMon/<version>` or
/// `TokenMon/unknown`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::user_agent;
///
/// assert_eq!(user_agent(Some("1.2.3")), "TokenMon/1.2.3");
/// assert_eq!(user_agent(None), "TokenMon/unknown");
/// ```
pub fn user_agent(app_version: Option<&str>) -> String {
    format!("TokenMon/{}", app_version.unwrap_or("unknown"))
}

fn value(name: &str, raw: &str) -> Result<HeaderValue, HeaderBuildError> {
    HeaderValue::from_str(raw).map_err(|_| HeaderBuildError(name.to_owned()))
}

fn sensitive(name: &str, raw: &str) -> Result<HeaderValue, HeaderBuildError> {
    let mut v = value(name, raw)?;
    v.set_sensitive(true);
    Ok(v)
}

/// Builds the standard provider headers (spec table 1.1.2).
///
/// `Cookie` (verbatim), `Authorization: Bearer <token>` and `Referer` are only
/// set when given and non-empty. `Accept: application/json` and `User-Agent`
/// are always set. Credentials are marked sensitive so `Debug` redacts them.
///
/// # Errors
///
/// [`HeaderBuildError`] when a value is not a legal header value.
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::build_headers;
///
/// let h = build_headers(Some("a=b"), Some("tok123"), Some("https://x.test"), "TokenMon/1").unwrap();
/// assert_eq!(h["cookie"], "a=b");
/// assert_eq!(h["authorization"], "Bearer tok123");
/// assert_eq!(h["accept"], "application/json");
/// assert_eq!(h["referer"], "https://x.test");
/// assert!(build_headers(Some(""), None, None, "TokenMon/1").unwrap().get("cookie").is_none());
/// ```
pub fn build_headers(
    cookie_header: Option<&str>,
    bearer_token: Option<&str>,
    referer: Option<&str>,
    user_agent: &str,
) -> Result<HeaderMap, HeaderBuildError> {
    let mut headers = HeaderMap::new();
    if let Some(c) = cookie_header.filter(|c| !c.is_empty()) {
        headers.insert(COOKIE, sensitive("Cookie", c)?);
    }
    if let Some(t) = bearer_token.filter(|t| !t.is_empty()) {
        headers.insert(
            AUTHORIZATION,
            sensitive("Authorization", &format!("Bearer {t}"))?,
        );
    }
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, value("User-Agent", user_agent)?);
    if let Some(r) = referer.filter(|r| !r.is_empty()) {
        headers.insert(REFERER, value("Referer", r)?);
    }
    Ok(headers)
}

/// Adds `(name, value)` pairs on top of `headers`, overriding earlier values.
///
/// # Errors
///
/// [`HeaderBuildError`] for an illegal name or value.
pub fn apply_extra_headers(
    headers: &mut HeaderMap,
    extra: &[(&str, &str)],
) -> Result<(), HeaderBuildError> {
    for (name, raw) in extra {
        let n = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| HeaderBuildError((*name).to_owned()))?;
        headers.insert(n, value(name, raw)?);
    }
    Ok(())
}

/// Maps an HTTP status to a [`UsageError`] without ever reading the body (so
/// the body cannot leak into a message). `None` for 2xx.
///
/// | Status | Result |
/// |---|---|
/// | 401, 403 | `Unauthorized` |
/// | 429 | `RateLimited` with the parsed `Retry-After` |
/// | other non-2xx | `BadResponse("HTTP <status>")` |
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use reqwest::{StatusCode, header::HeaderMap};
/// use tokenmon_core::{UsageError, net::map_error};
///
/// let h = HeaderMap::new();
/// assert_eq!(map_error(StatusCode::NO_CONTENT, &h, Utc::now()), None);
/// assert_eq!(map_error(StatusCode::UNAUTHORIZED, &h, Utc::now()), Some(UsageError::Unauthorized));
/// assert_eq!(
///     map_error(StatusCode::BAD_GATEWAY, &h, Utc::now()),
///     Some(UsageError::BadResponse("HTTP 502".into()))
/// );
/// ```
pub fn map_error(
    status: StatusCode,
    headers: &HeaderMap,
    now: DateTime<Utc>,
) -> Option<UsageError> {
    match status.as_u16() {
        401 | 403 => Some(UsageError::Unauthorized),
        429 => Some(UsageError::RateLimited {
            retry_after: retry_after_from_headers(headers, now),
        }),
        _ if status.is_success() => None,
        code => Some(UsageError::BadResponse(format!("HTTP {code}"))),
    }
}

/// The message used for a transient bot-protection block.
pub fn bot_challenge_message(status: StatusCode) -> String {
    format!(
        "Request blocked by a bot-protection check (HTTP {}). Retrying.",
        status.as_u16()
    )
}

/// Like [`map_error`], but a bot challenge (see [`is_bot_challenge`]) wins over
/// the status and becomes a transient `BadResponse`, whatever the status.
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use reqwest::{StatusCode, header::HeaderMap};
/// use tokenmon_core::{UsageError, net::response_error};
///
/// let e = response_error(StatusCode::FORBIDDEN, &HeaderMap::new(), b"<html>", Utc::now());
/// assert_eq!(
///     e,
///     Some(UsageError::BadResponse(
///         "Request blocked by a bot-protection check (HTTP 403). Retrying.".into()
///     ))
/// );
/// let json403 = response_error(StatusCode::FORBIDDEN, &HeaderMap::new(), b"{}", Utc::now());
/// assert_eq!(json403, Some(UsageError::Unauthorized));
/// ```
pub fn response_error(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    now: DateTime<Utc>,
) -> Option<UsageError> {
    if is_bot_challenge(status, headers, body) {
        return Some(UsageError::BadResponse(bot_challenge_message(status)));
    }
    map_error(status, headers, now)
}
