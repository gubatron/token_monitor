//! Body checks, HTTP status mapping and the Cursor auth configuration.

use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};
use tokenmon_provider_cursor::{
    CursorHttp, CursorSignIn, ForbiddenPolicy, RequestPolicy, cursor_auth_config, domain_matches,
    json_object, looks_like_login_page, map_status, parse_retry_after, reject_unauthorized_body,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CTX: ProviderErrorContext = ProviderErrorContext::Cursor;

/// A sign-in HTML page is an expired session.
#[test]
fn test_reject_unauthorized_body_treats_html_as_unauthorized() {
    let page = b"<!doctype html><html><body><h1>Sign in to Cursor</h1></body></html>";
    assert_eq!(
        reject_unauthorized_body(page, CTX).unwrap_err(),
        ProviderError::Unauthorized(CTX)
    );
}

/// Challenge and maintenance pages are transient bad responses.
#[test]
fn test_two_hundred_html_is_unauthorized_only_for_login_pages() {
    let challenge = b"<html>Just a moment... please login <div id=cf-chl></div></html>";
    let maintenance = b"<html><body>Down for maintenance</body></html>";
    for page in [&challenge[..], &maintenance[..]] {
        assert_eq!(
            json_object(page, CTX).unwrap_err(),
            ProviderError::BadResponse(CTX, "Unexpected HTML response".to_owned())
        );
    }
    assert!(!looks_like_login_page(challenge));
}

/// `not_authenticated` and `unauthorized` error strings are unauthorized.
#[test]
fn test_reject_unauthorized_body_maps_error_string() {
    for body in [
        &br#"{"error":"not_authenticated"}"#[..],
        br#"{"error":"Unauthorized request"}"#,
    ] {
        assert!(matches!(
            reject_unauthorized_body(body, CTX),
            Err(ProviderError::Unauthorized(_))
        ));
    }
    assert!(reject_unauthorized_body(br#"{"error":"boom"}"#, CTX).is_ok());
}

/// Truncated JSON is a bad response, not an auth failure.
#[test]
fn test_reject_unauthorized_body_treats_malformed_as_bad_response() {
    let e = reject_unauthorized_body(br#"{"individualUsage":"#, CTX).unwrap_err();
    assert!(matches!(e, ProviderError::BadResponse(_, _)));
}

/// A normal summary passes through.
#[test]
fn test_reject_unauthorized_body_passes_normal_summary() {
    assert!(reject_unauthorized_body(br#"{"individualUsage":{"plan":{}}}"#, CTX).is_ok());
}

/// Non-UTF-8 prefixes are handled (Latin-1 style scan).
#[test]
fn test_login_page_scan_tolerates_invalid_utf8() {
    let mut body = vec![0xff, 0xfe, b'<'];
    body.extend_from_slice(b"html>SIGN IN</html>");
    assert!(!looks_like_login_page(&body[1..2])); // no marker
    assert!(looks_like_login_page(&body[2..]));
}

/// The status table for Cursor: 403 is an expired session.
#[test]
fn test_status_mapping_for_cursor() {
    let p = RequestPolicy::CURSOR;
    assert!(map_status(200, false, None, b"{}", p).is_ok());
    assert_eq!(
        map_status(401, false, None, b"", p),
        Err(ProviderError::Unauthorized(CTX))
    );
    assert_eq!(
        map_status(403, false, None, b"{}", p),
        Err(ProviderError::Unauthorized(CTX))
    );
    assert_eq!(
        map_status(500, false, None, b"", p),
        Err(ProviderError::BadResponse(CTX, "HTTP 500".into()))
    );
    let rate = map_status(429, false, Some(30.0), b"", p).unwrap_err();
    assert_eq!(
        rate.to_string(),
        "Cursor is rate limiting requests. Retrying later."
    );
    assert_eq!(rate.usage_error().retry_after().unwrap().as_secs(), 30);
}

/// Grokbot's 403 is a transient custom error; challenges win over status.
#[test]
fn test_status_mapping_for_grokbot_and_challenges() {
    let p = RequestPolicy::GROKBOT;
    let forbidden = map_status(403, false, None, b"{}", p).unwrap_err();
    assert_eq!(
        forbidden.to_string(),
        "Grok Bot is not available on this Cursor account."
    );
    assert!(matches!(
        forbidden.usage_error(),
        UsageError::BadResponse(_)
    ));
    assert!(matches!(p.forbidden, ForbiddenPolicy::Custom(_)));
    let blocked = map_status(403, false, None, b"<html>blocked</html>", p).unwrap_err();
    assert!(
        blocked
            .to_string()
            .contains("bot-protection check (HTTP 403)")
    );
    assert!(map_status(200, true, None, b"{}", p).is_err());
}

/// `Retry-After` accepts sane seconds only.
#[test]
fn test_parse_retry_after() {
    assert_eq!(parse_retry_after("12"), Some(12.0));
    assert_eq!(parse_retry_after("NaN"), None);
    assert_eq!(parse_retry_after("-4"), None);
}

/// The transport sends cookie, accept, user agent and referer, and maps 401.
#[tokio::test]
async fn test_http_get_sends_shared_headers_and_maps_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .and(header("cookie", "WorkosCursorSessionToken=abc"))
        .and(header("accept", "application/json"))
        .and(header("user-agent", "TokenMon/test"))
        .and(header("referer", "https://cursor.com/bot"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/denied"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let http = CursorHttp::new(&server.uri(), "TokenMon/test");
    let cookie = "WorkosCursorSessionToken=abc";
    let policy = RequestPolicy::GROKBOT;
    let body = http
        .get("/ok", cookie, "https://cursor.com/bot", policy)
        .await
        .unwrap();
    assert_eq!(body, b"{}");
    let denied = http
        .get("/denied", cookie, "https://cursor.com/bot", policy)
        .await;
    assert_eq!(
        denied,
        Err(ProviderError::Unauthorized(ProviderErrorContext::Grokbot))
    );
}

/// A refused connection maps to a network error without leaking the URL.
#[tokio::test]
async fn test_http_transport_failure_is_network_error() {
    let http = CursorHttp::new("http://127.0.0.1:1", "TokenMon/test");
    let e = http
        .get("/x", "c=1", "https://cursor.com", RequestPolicy::CURSOR)
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Network(CTX, _)));
    assert!(!e.to_string().contains("127.0.0.1"));
}

/// Only the session token is an essential cookie.
#[test]
fn test_cursor_policy_stores_only_the_session_token() {
    let cfg = cursor_auth_config();
    assert!(cfg.is_essential("WorkosCursorSessionToken"));
    assert!(cfg.is_essential("workoscursorsessiontoken"));
    assert!(!cfg.is_essential("_ga"));
    assert!(!cfg.has_essential_allowlist() || cfg.essential_cookie_prefixes.is_empty());
    assert!(cfg.is_preferred_session_cookie("WorkosCursorSessionToken"));
    assert_eq!(cfg.store_filename_prefix, "cursor_auth_");
    assert_eq!(cfg.max_attempts, 4);
    assert_eq!(cfg.sign_out_hosts[0], "cursor.com");
    assert_eq!(cfg.hosts, cfg.sign_out_hosts);
    assert!(
        cfg.failure_message
            .starts_with("No Cursor session cookie found.")
    );
}

/// Identity hosts match exactly or by suffix, never by substring.
#[test]
fn test_auth_page_matches_identity_hosts_exactly_or_by_suffix() {
    assert!(CursorSignIn::is_auth_page("github.com", "/session"));
    assert!(CursorSignIn::is_auth_page("authenticator.cursor.sh", "/"));
    assert!(CursorSignIn::is_auth_page(
        "accounts.google.com",
        "/o/oauth2"
    ));
    assert!(CursorSignIn::is_auth_page("cursor.com", "/login"));
    assert!(!CursorSignIn::is_auth_page("github.com.evil.example", "/"));
    assert!(!CursorSignIn::is_auth_page("notgithub.com", "/"));
    assert!(!CursorSignIn::is_auth_page("cursor.com", "/dashboard"));
}

/// Cookie domains for Cursor.
#[test]
fn test_cursor_domain_filter() {
    for d in [
        "cursor.com",
        ".cursor.com",
        "www.cursor.com",
        "authenticator.cursor.sh",
    ] {
        assert!(CursorSignIn::is_cursor_domain(d), "{d}");
    }
    for d in ["opencode.ai", "grok.com", "evilcursor.com"] {
        assert!(!CursorSignIn::is_cursor_domain(d), "{d}");
    }
    assert!(!domain_matches("", &["cursor.com"]));
}

/// The return page is a cursor.com dashboard or settings page.
#[test]
fn test_return_page_and_auth_cookie_heuristic() {
    assert!(CursorSignIn::is_return_page(
        "www.cursor.com",
        "/dashboard/usage"
    ));
    assert!(CursorSignIn::is_return_page("cursor.com", "/settings"));
    assert!(!CursorSignIn::is_return_page(
        "evilcursor.com",
        "/dashboard"
    ));
    assert!(!CursorSignIn::is_return_page("cursor.com", "/pricing"));
    assert!(CursorSignIn::looks_like_auth_cookie(
        "WorkosCursorSessionToken"
    ));
    assert!(!CursorSignIn::looks_like_auth_cookie("_ga"));
}
