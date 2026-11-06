//! Core/Networking parity tests (spec 01 section 7): request building, status
//! mapping, Retry-After, challenge/login detection, Domain, and the wiremock
//! round trips of `ProviderHttp`. Hermetic: local mock server, `ManualClock`.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::json;
use tokenmon_core::net::{
    Domain, ProviderHttp, RequestOptions, build_headers, is_bot_challenge, is_unauthorized_message,
    json_object, looks_like_html, looks_like_login_page, map_error, parse_retry_after,
    response_error, set_cookie_headers,
};
use tokenmon_core::{ManualClock, ProviderError, ProviderErrorContext as Ctx, UsageError};
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const UA: &str = "TokenMon/test";
const CHALLENGE_403: &str = "Request blocked by a bot-protection check (HTTP 403). Retrying.";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2015, 10, 21, 7, 27, 0).unwrap()
}

fn hdrs(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        h.insert(*k, HeaderValue::from_static(v));
    }
    h
}

fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap()
}

fn http(ctx: Ctx) -> ProviderHttp {
    ProviderHttp::new(ctx, Some("test"), Arc::new(ManualClock::new(now()))).unwrap()
}

/// Cookie, Accept, Referer and User-Agent are set; empty values are skipped.
#[test]
fn test_builds_request_with_cookie_header() {
    let h = build_headers(Some("a=b; c=d"), None, Some("https://cursor.com"), UA).unwrap();
    assert_eq!(h["cookie"], "a=b; c=d");
    assert_eq!(h["accept"], "application/json");
    assert_eq!(h["referer"], "https://cursor.com");
    assert_eq!(h["user-agent"], UA);
    assert!(h.get("authorization").is_none());
    let empty = build_headers(Some(""), Some(""), Some(""), UA).unwrap();
    assert!(empty.get("cookie").is_none() && empty.get("referer").is_none());
    assert!(empty.get("authorization").is_none());
}

/// Bearer tokens become `Authorization: Bearer <token>`.
#[test]
fn test_adds_bearer_token_when_present() {
    let h = build_headers(None, Some("tok123"), None, UA).unwrap();
    assert_eq!(h["authorization"], "Bearer tok123");
}

/// A header value with a newline is rejected without echoing the secret.
#[test]
fn test_invalid_header_value_is_rejected_without_leaking_it() {
    let err = build_headers(Some("a=b\r\nX: y"), None, None, UA).unwrap_err();
    assert!(!err.to_string().contains("a=b"));
}

/// 401 and 403 are unauthorized.
#[test]
fn test_unauthorized_status_maps_to_unauthorized() {
    for code in [401, 403] {
        assert_eq!(
            map_error(status(code), &HeaderMap::new(), now()),
            Some(UsageError::Unauthorized)
        );
    }
}

/// 429 with `Retry-After: 120` keeps the hint.
#[test]
fn test_too_many_requests_maps_to_rate_limited_with_retry_after_seconds() {
    let e = map_error(status(429), &hdrs(&[("retry-after", "120")]), now()).unwrap();
    assert_eq!(
        e,
        UsageError::RateLimited {
            retry_after: Some(120.0)
        }
    );
    assert_eq!(e.retry_after().unwrap().as_secs(), 120);
}

/// 429 alone gives no hint.
#[test]
fn test_too_many_requests_without_retry_after() {
    assert_eq!(
        map_error(status(429), &HeaderMap::new(), now()),
        Some(UsageError::RateLimited { retry_after: None })
    );
}

/// HTTP-date `Retry-After` is relative to the injected now.
#[test]
fn test_retry_after_parses_http_date() {
    assert_eq!(
        parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now()),
        Some(60.0)
    );
}

/// Clamping, infinities and NaN, and garbage.
#[test]
fn test_retry_after_is_clamped() {
    for v in ["99999999999", "inf", "Fri, 31 Dec 9999 23:59:59 GMT"] {
        assert_eq!(parse_retry_after(v, now()), Some(3600.0), "{v}");
    }
    assert_eq!(parse_retry_after("-5", now()), Some(0.0));
    assert_eq!(
        parse_retry_after("Mon, 01 Jan 2001 00:00:00 GMT", now()),
        Some(0.0)
    );
    for v in ["nan", "", "   ", "soon", "12abc"] {
        assert_eq!(parse_retry_after(v, now()), None, "{v}");
    }
    assert_eq!(parse_retry_after(" 7 ", now()), Some(7.0));
    assert_eq!(parse_retry_after("1e2", now()), Some(100.0));
}

/// `from_usage` keeps `retry_after` and words the rate-limit message.
#[test]
fn test_rate_limited_provider_error_keeps_retry_after() {
    let e = ProviderError::from_usage(
        UsageError::RateLimited {
            retry_after: Some(30.0),
        },
        Ctx::Cursor,
    );
    assert_eq!(
        e.to_string(),
        "Cursor is rate limiting requests. Retrying later."
    );
    assert_eq!(e.usage_error().retry_after().unwrap().as_secs(), 30);
}

/// Other usage errors map one to one.
#[test]
fn test_from_usage_maps_other_cases_one_to_one() {
    assert_eq!(
        ProviderError::from_usage(UsageError::NotSignedIn, Ctx::Grok),
        ProviderError::NotSignedIn(Ctx::Grok)
    );
    assert_eq!(
        ProviderError::from_usage(UsageError::Unauthorized, Ctx::Grok),
        ProviderError::Unauthorized(Ctx::Grok)
    );
    assert_eq!(
        ProviderError::from_usage(UsageError::Network("x".into()), Ctx::Claude),
        ProviderError::Network(Ctx::Claude, "x".into())
    );
    assert_eq!(
        ProviderError::from_usage(UsageError::BadResponse("y".into()), Ctx::Claude),
        ProviderError::BadResponse(Ctx::Claude, "y".into())
    );
}

/// 500 gives `HTTP 500`.
#[test]
fn test_non_2xx_maps_to_bad_response() {
    assert_eq!(
        map_error(status(500), &HeaderMap::new(), now()),
        Some(UsageError::BadResponse("HTTP 500".into()))
    );
}

/// The body is never consulted by `map_error`, so its secret cannot leak.
#[test]
fn test_non_2xx_does_not_leak_response_body() {
    let e = response_error(
        status(500),
        &HeaderMap::new(),
        b"{\"secret\":\"hunter2\"}",
        now(),
    );
    assert!(!format!("{e:?}").contains("hunter2"));
}

/// 200 and 204 are not errors.
#[test]
fn test_no_error_on_success_range() {
    for code in [200, 204] {
        assert_eq!(map_error(status(code), &HeaderMap::new(), now()), None);
    }
}

/// 403 with HTML is a transient challenge.
#[test]
fn test_forbidden_html_is_transient_challenge() {
    assert_eq!(
        response_error(
            status(403),
            &HeaderMap::new(),
            b"<html>blocked</html>",
            now()
        ),
        Some(UsageError::BadResponse(CHALLENGE_403.into()))
    );
}

/// `cf-mitigated` makes any response a challenge, even JSON.
#[test]
fn test_cloudflare_mitigated_header_is_transient_challenge() {
    let h = hdrs(&[("cf-mitigated", "challenge")]);
    assert_eq!(
        response_error(status(403), &h, b"{}", now()),
        Some(UsageError::BadResponse(CHALLENGE_403.into()))
    );
    assert!(is_bot_challenge(status(200), &h, b"{}"));
}

/// 403 JSON and 401 HTML still reject the session.
#[test]
fn test_forbidden_json_and_unauthorized_still_reject_session() {
    assert_eq!(
        response_error(status(403), &HeaderMap::new(), b"{\"a\":1}", now()),
        Some(UsageError::Unauthorized)
    );
    assert_eq!(
        response_error(status(401), &HeaderMap::new(), b"<html>", now()),
        Some(UsageError::Unauthorized)
    );
}

/// `json_object` on a 200: login page is unauthorized, challenge and
/// maintenance pages are "Unexpected HTML response".
#[test]
fn test_two_hundred_html_is_unauthorized_only_for_login_pages() {
    let login = b"<html><body><form><input type=\"password\"></form></body></html>";
    assert_eq!(
        json_object(login, Ctx::Cursor).unwrap_err(),
        ProviderError::Unauthorized(Ctx::Cursor)
    );
    let challenge = b"<html><title>Just a moment...</title> sign in <script>cf_chl_opt</script>";
    let maintenance = b"<html><body>Down for maintenance</body></html>";
    for body in [&challenge[..], &maintenance[..]] {
        assert_eq!(
            json_object(body, Ctx::Cursor).unwrap_err(),
            ProviderError::BadResponse(Ctx::Cursor, "Unexpected HTML response".into())
        );
    }
    for body in [&b"nope"[..], b"[1,2]", b"\"s\"", b""] {
        assert_eq!(
            json_object(body, Ctx::Cursor).unwrap_err(),
            ProviderError::BadResponse(Ctx::Cursor, "Malformed response body".into())
        );
    }
    assert_eq!(json_object(b"{\"a\":1}", Ctx::Cursor).unwrap()["a"], 1);
}

/// Login detection is bounded to the first 64 KiB and ignores non-HTML.
#[test]
fn test_login_page_detection_boundaries() {
    assert!(!looks_like_login_page(b"{\"login\":1}"));
    let mut late = b"<html>".to_vec();
    late.extend(std::iter::repeat_n(b' ', 70_000));
    late.extend_from_slice(b"sign in");
    assert!(!looks_like_login_page(&late));
    assert!(looks_like_login_page(b"<HTML>SIGN-IN</HTML>"));
    assert!(looks_like_login_page(b"<a href='/authkit'>"));
    assert!(!looks_like_login_page(&[
        0xff, 0xfe, b'<', b'l', b'o', b'g', b'i', b'n'
    ]));
}

/// HTML sniffing: whitespace trimmed, 512-byte window, invalid UTF-8 rejected,
/// and a character cut by the window is tolerated.
#[test]
fn test_looks_like_html_window_and_encoding() {
    assert!(looks_like_html(b" \n\t<x"));
    assert!(!looks_like_html(b""));
    assert!(!looks_like_html(b"x<"));
    let mut cut = b"<".to_vec();
    cut.extend(std::iter::repeat_n(b'a', 510));
    cut.extend_from_slice("\u{e9}tail".as_bytes()); // 0xC3 lands on byte 512
    assert!(looks_like_html(&cut));
}

/// Unauthorized message sniffing.
#[test]
fn test_is_unauthorized_message() {
    assert!(is_unauthorized_message("NOT_AUTHENTICATED"));
    assert!(is_unauthorized_message("User Unauthorized"));
    assert!(!is_unauthorized_message("rate limit"));
}

/// Domain matching accepts hosts and subdomains, rejects lookalikes.
#[test]
fn test_domain_matches_accepted_hosts() {
    let grok = ["grok.com"];
    assert!(Domain::matches("grok.com", &grok));
    assert!(Domain::matches(".grok.com", &grok));
    assert!(Domain::matches("www.grok.com", &grok));
    assert!(Domain::matches("WWW.Grok.COM.", &grok));
    assert!(!Domain::matches("grok.com.evil.example", &grok));
    assert!(!Domain::matches("evilgrok.com", &grok));
    assert!(!Domain::matches("", &grok));
    assert!(!Domain::matches("notopencode.ai", &["opencode.ai"]));
    assert!(Domain::matches("x.com", &["grok.com", "x.com"]));
    assert!(!Domain::matches("grok.com", &[]));
}

/// Every `UsageError` has a non-empty description.
#[test]
fn test_usage_error_localized_descriptions() {
    for e in [
        UsageError::NotSignedIn,
        UsageError::Unauthorized,
        UsageError::Network("n".into()),
        UsageError::BadResponse("b".into()),
        UsageError::RateLimited { retry_after: None },
    ] {
        assert_ne!(e.to_string(), "");
    }
}

/// Equality includes the payload.
#[test]
fn test_usage_error_equality() {
    assert_eq!(
        UsageError::Network("a".into()),
        UsageError::Network("a".into())
    );
    assert_ne!(
        UsageError::Network("a".into()),
        UsageError::Network("b".into())
    );
    assert_ne!(
        UsageError::RateLimited {
            retry_after: Some(1.0)
        },
        UsageError::RateLimited { retry_after: None }
    );
}

/// `ProviderError` unauthorized/notSignedIn map to the matching usage errors.
#[test]
fn test_client_mappings_to_common_auth_cases() {
    for ctx in [Ctx::Cursor, Ctx::OpenCode, Ctx::Grok] {
        assert_eq!(
            ProviderError::Unauthorized(ctx).usage_error(),
            UsageError::Unauthorized
        );
        assert_eq!(
            ProviderError::NotSignedIn(ctx).usage_error(),
            UsageError::NotSignedIn
        );
    }
}

/// `Set-Cookie` values stay separate.
#[test]
fn test_set_cookie_headers_collects_each_line() {
    let mut h = HeaderMap::new();
    h.append("set-cookie", HeaderValue::from_static("a=1"));
    h.append("Set-Cookie", HeaderValue::from_static("b=2; Secure"));
    assert_eq!(set_cookie_headers(&h), ["a=1", "b=2; Secure"]);
}

/// Path resolution per RFC 3986.
#[test]
fn test_resolve_paths() {
    let r = |p, b| ProviderHttp::resolve(p, b).map(|u| u.to_string());
    assert_eq!(
        r("key", "https://openrouter.ai/api/v1").unwrap(),
        "https://openrouter.ai/api/v1/key"
    );
    assert_eq!(
        r("key", "https://openrouter.ai/api/v1/").unwrap(),
        "https://openrouter.ai/api/v1/key"
    );
    assert_eq!(
        r("/api/x", "https://openrouter.ai/api/v1").unwrap(),
        "https://openrouter.ai/api/x"
    );
    assert_eq!(
        r("https://a.test/z", "https://b.test").unwrap(),
        "https://a.test/z"
    );
    assert_eq!(
        r("key", "https://openrouter.ai").unwrap(),
        "https://openrouter.ai/key"
    );
    assert!(r("x", "not a url").is_none());
}

/// Wire: headers applied, UA set, body returned.
#[tokio::test]
async fn test_get_sends_standard_headers_and_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/usage"))
        .and(header("cookie", "s=1"))
        .and(header("authorization", "Bearer t"))
        .and(header("accept", "application/json"))
        .and(header("user-agent", "TokenMon/test"))
        .and(header("referer", "https://r.test"))
        .and(header("origin", "https://o.test"))
        .and(header("x-extra", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;
    let opts = RequestOptions {
        cookie_header: Some("s=1"),
        bearer_token: Some("t"),
        referer: Some("https://r.test"),
        origin: Some("https://o.test"),
        headers: &[("x-extra", "1")],
    };
    let body = http(Ctx::Claude)
        .get(&server.uri(), "api/usage", opts)
        .await
        .unwrap();
    assert_eq!(body, b"ok");
}

/// Wire: a Set-Cookie answer is never replayed (no cookie jar).
#[tokio::test]
async fn test_provider_session_has_no_cookie_jar_or_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "jar=1; Path=/")
                .insert_header("cache-control", "max-age=3600")
                .set_body_string("x"),
        )
        .expect(2) // not served from a cache
        .mount(&server)
        .await;
    let client = http(Ctx::Grok);
    let first = client
        .get_with_response(&server.uri(), "a", RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(first.set_cookie_headers(), ["jar=1; Path=/"]);
    client
        .get(&server.uri(), "a", RequestOptions::default())
        .await
        .unwrap();
    let received = server.received_requests().await.unwrap();
    assert!(received.iter().all(|r| !r.headers.contains_key("cookie")));
}

/// Wire: status mapping with provider wording and Retry-After.
#[tokio::test]
async fn test_status_errors_over_the_wire() {
    let server = MockServer::start().await;
    Mock::given(path("/limited"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;
    Mock::given(path("/boom"))
        .respond_with(ResponseTemplate::new(500).set_body_string("secret-body"))
        .mount(&server)
        .await;
    Mock::given(path("/auth"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let c = http(Ctx::Cursor);
    let o = RequestOptions::default();
    let e = c.get(&server.uri(), "limited", o).await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "Cursor is rate limiting requests. Retrying later."
    );
    assert_eq!(e.usage_error().retry_after().unwrap().as_secs(), 30);
    let e = c.get(&server.uri(), "boom", o).await.unwrap_err();
    assert_eq!(
        e,
        ProviderError::BadResponse(Ctx::Cursor, "HTTP 500".into())
    );
    assert!(!e.to_string().contains("secret-body"));
    let e = c.get(&server.uri(), "auth", o).await.unwrap_err();
    assert_eq!(e, ProviderError::Unauthorized(Ctx::Cursor));
}

/// Wire: Grokbot's plain 403 is its custom message, but a challenge 403 stays
/// a transient bad response.
#[tokio::test]
async fn test_forbidden_override_skips_challenges() {
    let server = MockServer::start().await;
    Mock::given(path("/plain"))
        .respond_with(ResponseTemplate::new(403).set_body_string("{}"))
        .mount(&server)
        .await;
    Mock::given(path("/html"))
        .respond_with(ResponseTemplate::new(403).set_body_string("<html>x</html>"))
        .mount(&server)
        .await;
    let c = http(Ctx::Grokbot);
    let o = RequestOptions::default();
    let msg = "Grok Bot is not available on this Cursor account.";
    let e = c.get(&server.uri(), "plain", o).await.unwrap_err();
    assert_eq!(e.to_string(), msg);
    assert_eq!(e.usage_error(), UsageError::BadResponse(msg.into()));
    let e = c.get(&server.uri(), "html", o).await.unwrap_err();
    assert_eq!(
        e.usage_error(),
        UsageError::BadResponse(CHALLENGE_403.into())
    );
    // Other contexts treat the plain 403 as unauthorized.
    let e = http(Ctx::Cursor)
        .get(&server.uri(), "plain", o)
        .await
        .unwrap_err();
    assert_eq!(e, ProviderError::Unauthorized(Ctx::Cursor));
}

/// Wire: POST JSON sets the content type and body; redirects are followed.
#[tokio::test]
async fn test_post_json_and_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/p"))
        .and(header("content-type", "application/json"))
        .and(header_exists("accept"))
        .respond_with(ResponseTemplate::new(307).insert_header("location", "/final"))
        .mount(&server)
        .await;
    Mock::given(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"ok\":true}"))
        .mount(&server)
        .await;
    let r = http(Ctx::OpenCode)
        .post_json(
            &server.uri(),
            "/p",
            &json!({"a": 1}),
            RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(r.status, StatusCode::OK);
    let posted = &server.received_requests().await.unwrap()[0];
    assert_eq!(posted.body, b"{\"a\":1}");
}

/// Wire: transport failure is a Network error without the URL.
#[tokio::test]
async fn test_transport_failure_is_network_error_without_url() {
    let e = http(Ctx::Claude)
        .get(
            "http://127.0.0.1:1",
            "secret-path?token=abc",
            RequestOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Network(Ctx::Claude, _)));
    assert!(!e.to_string().contains("abc"));
}

/// An invalid path yields `Invalid path <path>`.
#[tokio::test]
async fn test_invalid_base_reports_invalid_path() {
    let e = http(Ctx::Claude)
        .get("not a url", "x", RequestOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        e,
        ProviderError::BadResponse(Ctx::Claude, "Invalid path x".into())
    );
}
