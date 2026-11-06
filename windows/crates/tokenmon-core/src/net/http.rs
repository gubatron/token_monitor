//! The HTTP client every provider goes through (spec 01 sections 1.1.1 to 1.1.3).
//!
//! One `reqwest::Client` per [`ProviderHttp`]: no cookie jar, no cache layer,
//! default redirect policy, 15 s connect timeout and 60 s request timeout.
//! Credentials travel only in the explicit `Cookie` / `Authorization` headers
//! built by [`build_headers`]; `Set-Cookie` is never stored automatically.

use std::error::Error as _;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Map, Value};
use url::Url;

use crate::clock::Clock;

use super::detect::{is_bot_challenge, looks_like_html, looks_like_login_page};
use super::error::{ProviderError, ProviderErrorContext, UsageError};
use super::request::{
    HeaderBuildError, apply_extra_headers, build_headers, response_error, user_agent,
};

/// Per-request knobs; every field is optional (spec `get(...)` parameters).
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::RequestOptions;
///
/// let opts = RequestOptions {
///     cookie_header: Some("sessionKey=abc"),
///     referer: Some("https://claude.ai"),
///     ..RequestOptions::default()
/// };
/// assert!(opts.bearer_token.is_none());
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestOptions<'a> {
    /// `Cookie` header, verbatim.
    pub cookie_header: Option<&'a str>,
    /// Bearer token for `Authorization`.
    pub bearer_token: Option<&'a str>,
    /// `Referer` header.
    pub referer: Option<&'a str>,
    /// `Origin` header.
    pub origin: Option<&'a str>,
    /// Extra headers; they override the standard ones.
    pub headers: &'a [(&'a str, &'a str)],
    /// Overrides the client's response-size cap for this request
    /// ([`DEFAULT_MAX_RESPONSE_BYTES`] unless changed with
    /// [`ProviderHttp::with_max_response_bytes`]).
    pub max_response_bytes: Option<usize>,
}

/// Default cap on a buffered response body: 16 MiB.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// A response body grew past the allowed size and was abandoned unread.
///
/// Surfaces to callers as `BadResponse("Response too large")`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("response body exceeds the {limit} byte limit")]
pub struct ResponseTooLarge {
    /// The cap that was exceeded, in bytes.
    pub limit: usize,
}

/// Checks that appending `incoming` bytes to a body of `held` bytes stays
/// within `limit`, so a reader never buffers more than `limit` plus one chunk.
///
/// # Errors
///
/// [`ResponseTooLarge`] when the total would exceed `limit`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::http::{ResponseTooLarge, ensure_room};
///
/// assert_eq!(ensure_room(2, 1, 3), Ok(()));
/// assert_eq!(ensure_room(2, 2, 3), Err(ResponseTooLarge { limit: 3 }));
/// ```
pub fn ensure_room(held: usize, incoming: usize, limit: usize) -> Result<(), ResponseTooLarge> {
    if held.saturating_add(incoming) > limit {
        Err(ResponseTooLarge { limit })
    } else {
        Ok(())
    }
}

/// A completed HTTP exchange whose status was acceptable to the caller.
#[derive(Clone)]
pub struct HttpResponse {
    /// Final status (after redirects).
    pub status: StatusCode,
    /// Response headers.
    pub headers: HeaderMap,
    /// Full body.
    pub body: Vec<u8>,
}

impl std::fmt::Debug for HttpResponse {
    /// Header values (`Set-Cookie`, `Authorization`) and the body can carry
    /// credentials: only header names and the body size are printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.keys().map(HeaderName::as_str).collect();
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("header_names", &names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl HttpResponse {
    /// Every `Set-Cookie` value, each as its own string (see [`set_cookie_headers`]).
    pub fn set_cookie_headers(&self) -> Vec<String> {
        set_cookie_headers(&self.headers)
    }
}

/// Every `Set-Cookie` header value, one entry per header line.
///
/// Foundation folds repeated headers into one comma-joined value; here they stay
/// separate, and cookie parsers must still accept comma-folded input.
///
/// # Examples
///
/// ```
/// use reqwest::header::{HeaderMap, HeaderValue, SET_COOKIE};
/// use tokenmon_core::net::set_cookie_headers;
///
/// let mut h = HeaderMap::new();
/// h.append(SET_COOKIE, HeaderValue::from_static("a=1; Path=/"));
/// h.append(SET_COOKIE, HeaderValue::from_static("b=2"));
/// assert_eq!(set_cookie_headers(&h), vec!["a=1; Path=/", "b=2"]);
/// ```
pub fn set_cookie_headers(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(SET_COOKIE)
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .collect()
}

/// Splits one comma-folded `Set-Cookie` value (Foundation joins repeated
/// headers with `, `) back into individual cookies.
///
/// A comma separates cookies only when the text after it, up to the next `;`
/// or `,`, contains `=`; this keeps `Expires=Wed, 21 Oct 2015 ...` intact.
/// Total over arbitrary input; empty pieces are dropped.
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::split_folded_set_cookie;
///
/// let v = "a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/, b=2; Secure";
/// assert_eq!(
///     split_folded_set_cookie(v),
///     ["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/", "b=2; Secure"]
/// );
/// ```
pub fn split_folded_set_cookie(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, _) in value.match_indices(',') {
        let rest = &value[i + 1..];
        let seg_end = rest.find([';', ',']).unwrap_or(rest.len());
        if rest[..seg_end].contains('=') {
            parts.push(&value[start..i]);
            start = i + 1;
        }
    }
    parts.push(&value[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Runs `request` until it finishes or `cancel` resolves, whichever is first.
///
/// When `cancel` wins the request future is dropped (aborting the transfer)
/// and [`ProviderError::Cancelled`] is returned.
///
/// # Examples
///
/// ```
/// use tokenmon_core::{ProviderError, net::with_cancellation};
///
/// # futures::executor::block_on(async {
/// let r: Result<u8, _> = with_cancellation(async {}, std::future::pending()).await;
/// assert_eq!(r, Err(ProviderError::Cancelled));
/// let r = with_cancellation(std::future::pending(), async { Ok::<_, ProviderError>(1) }).await;
/// assert_eq!(r, Ok(1));
/// # });
/// ```
pub async fn with_cancellation<T>(
    cancel: impl Future<Output = ()>,
    request: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    use futures::future::{Either, select};
    match select(Box::pin(cancel), Box::pin(request)).await {
        Either::Left(_) => Err(ProviderError::Cancelled),
        Either::Right((result, _)) => result,
    }
}

/// Parses a body that must be a JSON **object** (spec `ProviderHTTP.jsonObject`).
///
/// # Errors
///
/// * HTML that looks like a login page: [`ProviderError::Unauthorized`].
/// * Other HTML: `BadResponse("Unexpected HTML response")`.
/// * Anything else (including valid JSON that is not an object):
///   `BadResponse("Malformed response body")`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::{ProviderError, ProviderErrorContext as C};
/// use tokenmon_core::net::json_object;
///
/// assert!(json_object(br#"{"a":1}"#, C::Claude).is_ok());
/// assert_eq!(
///     json_object(b"<html>Sign in</html>", C::Claude).unwrap_err(),
///     ProviderError::Unauthorized(C::Claude)
/// );
/// assert_eq!(
///     json_object(b"[1]", C::Claude).unwrap_err().to_string(),
///     "Claude response error: Malformed response body"
/// );
/// ```
pub fn json_object(
    body: &[u8],
    context: ProviderErrorContext,
) -> Result<Map<String, Value>, ProviderError> {
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
        return Ok(map);
    }
    if looks_like_html(body) {
        if looks_like_login_page(body) {
            return Err(ProviderError::Unauthorized(context));
        }
        return Err(ProviderError::BadResponse(
            context,
            "Unexpected HTML response".into(),
        ));
    }
    Err(ProviderError::BadResponse(
        context,
        "Malformed response body".into(),
    ))
}

impl ProviderHttp {
    /// Resolves `path` against `base` as an RFC 3986 relative reference, appending
    /// `/` to `base` first when missing.
    ///
    /// Returns `None` when `base` is not a URL or the path cannot be resolved.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::net::ProviderHttp;
    ///
    /// let r = |p, b| ProviderHttp::resolve(p, b).map(|u| u.to_string());
    /// assert_eq!(r("key", "https://openrouter.ai/api/v1").unwrap(), "https://openrouter.ai/api/v1/key");
    /// assert_eq!(r("/api/x", "https://openrouter.ai/api/v1").unwrap(), "https://openrouter.ai/api/x");
    /// assert_eq!(r("https://a.test/z", "https://b.test").unwrap(), "https://a.test/z");
    /// ```
    pub fn resolve(path: &str, base: &str) -> Option<Url> {
        let base = if base.ends_with('/') {
            base.to_owned()
        } else {
            format!("{base}/")
        };
        Url::parse(&base).ok()?.join(path).ok()
    }
}

/// HTTP client bound to one provider's wording and one clock.
///
/// Every `get`/`post_json` call applies the standard headers, maps transport
/// failures to `Network`, challenges and statuses via [`response_error`], and
/// wraps the result in the provider's [`ProviderErrorContext`].
///
/// A cancelled request is a dropped future: there is no error for it.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use tokenmon_core::{ProviderErrorContext, SystemClock};
/// use tokenmon_core::net::ProviderHttp;
///
/// let http = ProviderHttp::new(ProviderErrorContext::Cursor, Some("0.1.0"), Arc::new(SystemClock))
///     .expect("client builds");
/// assert_eq!(http.context(), ProviderErrorContext::Cursor);
/// ```
#[derive(Clone)]
pub struct ProviderHttp {
    client: Client,
    context: ProviderErrorContext,
    user_agent: String,
    clock: Arc<dyn Clock>,
    max_response_bytes: usize,
}

impl std::fmt::Debug for ProviderHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderHttp")
            .field("context", &self.context)
            .field("user_agent", &self.user_agent)
            .finish_non_exhaustive()
    }
}

fn flatten(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

impl ProviderHttp {
    /// Builds the shared ephemeral client: no cookie jar, no cache, default
    /// redirects, 15 s connect and 60 s request timeouts.
    ///
    /// `app_version` feeds `User-Agent: TokenMon/<version>` (`unknown` if `None`).
    ///
    /// # Errors
    ///
    /// `Network` when the TLS backend cannot be initialised.
    pub fn new(
        context: ProviderErrorContext,
        app_version: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| ProviderError::Network(context, flatten(&e)))?;
        Ok(Self::with_client(client, context, app_version, clock))
    }

    /// Wraps an existing client (tests, or sharing a connection pool).
    pub fn with_client(
        client: Client,
        context: ProviderErrorContext,
        app_version: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            client,
            context,
            user_agent: user_agent(app_version),
            clock,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Changes the default response-size cap (per-request
    /// [`RequestOptions::max_response_bytes`] still wins).
    #[must_use]
    pub fn with_max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }

    /// The provider wording this client reports errors in.
    pub fn context(&self) -> ProviderErrorContext {
        self.context
    }

    /// GET `path` under `base` and return the body of a 2xx answer.
    ///
    /// # Errors
    ///
    /// See [`ProviderHttp::get_with_response`].
    pub async fn get(
        &self,
        base: &str,
        path: &str,
        options: RequestOptions<'_>,
    ) -> Result<Vec<u8>, ProviderError> {
        Ok(self.get_with_response(base, path, options).await?.body)
    }

    /// GET `path` under `base` and return the whole 2xx response.
    ///
    /// # Errors
    ///
    /// * `BadResponse("Invalid path <path>")` for an unresolvable path.
    /// * `Network` for transport failures (URL stripped from the message).
    /// * The provider's forbidden message ([`ProviderErrorContext::forbidden_message`])
    ///   for a 403 that is not a bot challenge, as `Custom` over `BadResponse`.
    /// * Otherwise [`response_error`] wrapped with [`ProviderError::from_usage`].
    pub async fn get_with_response(
        &self,
        base: &str,
        path: &str,
        options: RequestOptions<'_>,
    ) -> Result<HttpResponse, ProviderError> {
        self.perform(Method::GET, base, path, options, None).await
    }

    /// POST `json` (as `Content-Type: application/json`) to `path` under `base`.
    ///
    /// # Errors
    ///
    /// Same as [`ProviderHttp::get_with_response`].
    pub async fn post_json(
        &self,
        base: &str,
        path: &str,
        json: &Value,
        options: RequestOptions<'_>,
    ) -> Result<HttpResponse, ProviderError> {
        self.perform(Method::POST, base, path, options, Some(json))
            .await
    }

    fn headers(&self, options: &RequestOptions<'_>) -> Result<HeaderMap, HeaderBuildError> {
        let mut headers = build_headers(
            options.cookie_header,
            options.bearer_token,
            options.referer,
            &self.user_agent,
        )?;
        apply_extra_headers(&mut headers, options.headers)?;
        if let Some(origin) = options.origin.filter(|o| !o.is_empty()) {
            let v =
                HeaderValue::from_str(origin).map_err(|_| HeaderBuildError("Origin".to_owned()))?;
            headers.insert(HeaderName::from_static("origin"), v);
        }
        Ok(headers)
    }

    async fn perform(
        &self,
        method: Method,
        base: &str,
        path: &str,
        options: RequestOptions<'_>,
        json: Option<&Value>,
    ) -> Result<HttpResponse, ProviderError> {
        let ctx = self.context;
        let bad = |m: String| ProviderError::BadResponse(ctx, m);
        let url = Self::resolve(path, base).ok_or_else(|| bad(format!("Invalid path {path}")))?;
        let mut headers = self.headers(&options).map_err(|e| bad(e.to_string()))?;
        let mut request = self.client.request(method, url);
        if let Some(body) = json {
            let bytes = serde_json::to_vec(body).map_err(|e| bad(e.to_string()))?;
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            request = request.body(bytes);
        }
        let network = |e: reqwest::Error| ProviderError::Network(ctx, flatten(&e.without_url()));
        let limit = options
            .max_response_bytes
            .unwrap_or(self.max_response_bytes);
        let mut response = request.headers(headers).send().await.map_err(network)?;
        let status = response.status();
        let response_headers = response.headers().clone();
        let too_large = || ProviderError::BadResponse(ctx, "Response too large".to_owned());
        if response
            .content_length()
            .is_some_and(|n| usize::try_from(n).map_or(true, |n| n > limit))
        {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            ensure_room(body.len(), chunk.len(), limit).map_err(|_| too_large())?;
            body.extend_from_slice(&chunk);
        }
        self.check(status, &response_headers, &body)?;
        Ok(HttpResponse {
            status,
            headers: response_headers,
            body,
        })
    }

    fn check(
        &self,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), ProviderError> {
        let ctx = self.context;
        if status == StatusCode::FORBIDDEN
            && !is_bot_challenge(status, headers, body)
            && let Some(message) = ctx.forbidden_message()
        {
            return Err(ProviderError::Custom {
                message: message.to_owned(),
                usage: UsageError::BadResponse(message.to_owned()),
            });
        }
        match response_error(status, headers, body, self.clock.now()) {
            Some(usage) => Err(ProviderError::from_usage(usage, ctx)),
            None => Ok(()),
        }
    }
}
