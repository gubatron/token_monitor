//! HTTP plumbing shared by Cursor and Grokbot (spec 02 sections 2.2, 3.2).
//!
//! Both providers call `cursor.com` with the same cookie header, `Referer` and
//! `Origin`. The transport, status mapping, bot-challenge detection, the
//! Grokbot 403 wording and the response-body checks all live in
//! [`tokenmon_core::net`]; [`CursorHttp`] only binds a [`ProviderHttp`] (which
//! carries the provider's [`ProviderErrorContext`]) to a base URL and the
//! cookie header, and [`reject_unauthorized_body`] adds the one rule that is
//! specific to `cursor.com` JSON.
//!
//! Secrets: the cookie header is passed per call and never stored, logged or
//! included in any error text.

use std::sync::Arc;

use serde_json::{Map, Value};
use tokenmon_core::net::{ProviderHttp, RequestOptions, json_object};
use tokenmon_core::{Clock, ProviderError, ProviderErrorContext};

use crate::auth::CURSOR_COOKIE_HOSTS;

/// Production base URL.
pub const CURSOR_BASE_URL: &str = "https://cursor.com";

/// Largest response accepted: usage summaries are a few KB, event pages a few
/// MiB at most.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// A [`ProviderHttp`] bound to a `cursor.com` base URL.
///
/// Cheap to clone. The wrapped client fixes the error wording
/// ([`ProviderErrorContext::Cursor`] or [`ProviderErrorContext::Grokbot`]); the
/// context also decides what a non-challenge 403 means (core maps it to
/// `Unauthorized` for Cursor and to the "not available" message for Grokbot).
#[derive(Debug, Clone)]
pub struct CursorHttp {
    http: ProviderHttp,
    base_url: String,
}

impl CursorHttp {
    /// Wraps `http`, resolving paths against `base_url`.
    pub fn new(http: ProviderHttp, base_url: &str) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// A fresh client for `base_url`; `app_version` feeds the `User-Agent`.
    ///
    /// # Errors
    /// `Network` when the TLS backend cannot be initialised.
    pub fn with_base_url(
        context: ProviderErrorContext,
        base_url: &str,
        app_version: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProviderError> {
        Ok(Self::new(
            ProviderHttp::for_hosts(
                context,
                app_version,
                clock,
                &CURSOR_COOKIE_HOSTS.map(str::to_owned),
            )?,
            base_url,
        ))
    }

    /// Production client for [`CURSOR_BASE_URL`].
    ///
    /// # Errors
    /// `Network` when the TLS backend cannot be initialised.
    pub fn production(
        context: ProviderErrorContext,
        app_version: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProviderError> {
        Self::with_base_url(context, CURSOR_BASE_URL, app_version, clock)
    }

    /// The provider wording this client reports errors in.
    pub fn context(&self) -> ProviderErrorContext {
        self.http.context()
    }

    /// `GET <base><path>` with the cookie and `Referer`; returns the 2xx body.
    ///
    /// # Errors
    /// See [`ProviderHttp::get_with_response`].
    pub async fn get(
        &self,
        path: &str,
        cookie: &str,
        referer: &str,
    ) -> Result<Vec<u8>, ProviderError> {
        self.http
            .get(&self.base_url, path, Self::options(cookie, referer, None))
            .await
    }

    /// `POST <base><path>` with a JSON body and `Origin: https://cursor.com`;
    /// returns the 2xx body.
    ///
    /// # Errors
    /// See [`ProviderHttp::get_with_response`].
    pub async fn post_json(
        &self,
        path: &str,
        cookie: &str,
        referer: &str,
        body: &Value,
    ) -> Result<Vec<u8>, ProviderError> {
        let options = Self::options(cookie, referer, Some(CURSOR_BASE_URL));
        Ok(self
            .http
            .post_json(&self.base_url, path, body, options)
            .await?
            .body)
    }

    fn options<'a>(
        cookie: &'a str,
        referer: &'a str,
        origin: Option<&'a str>,
    ) -> RequestOptions<'a> {
        RequestOptions {
            cookie_header: Some(cookie),
            referer: Some(referer),
            origin,
            max_response_bytes: Some(MAX_RESPONSE_BYTES),
            ..RequestOptions::default()
        }
    }
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
