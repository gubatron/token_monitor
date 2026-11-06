//! Body checks, HTTP status mapping and the Cursor auth configuration.

use std::sync::Arc;

use tokenmon_core::net::{json_object, looks_like_login_page};
use tokenmon_core::{FixedClock, ProviderError, ProviderErrorContext, UsageError};
use tokenmon_provider_cursor::{
    CursorHttp, CursorSignIn, cursor_auth_config, reject_unauthorized_body,
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

/// A `CursorHttp` for `uri` with the given provider wording.
fn http_for(ctx: ProviderErrorContext, uri: &str) -> CursorHttp {
    CursorHttp::with_base_url(ctx, uri, Some("test"), Arc::new(FixedClock::from_unix(0)))
        .expect("client")
}

/// Mounts a GET `/x` answering `template` and returns the outcome of fetching it.
async fn get_x(
    ctx: ProviderErrorContext,
    template: ResponseTemplate,
) -> Result<Vec<u8>, ProviderError> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .respond_with(template)
        .mount(&server)
        .await;
    http_for(ctx, &server.uri())
        .get("/x", "c=1", "https://cursor.com")
        .await
}

/// The status table for Cursor: 403 is an expired session.
#[tokio::test]
async fn test_status_mapping_for_cursor() {
    let ok = get_x(CTX, ResponseTemplate::new(200).set_body_string("{}")).await;
    assert_eq!(ok.unwrap(), b"{}");
    for status in [401, 403] {
        assert_eq!(
            get_x(CTX, ResponseTemplate::new(status).set_body_string("{}")).await,
            Err(ProviderError::Unauthorized(CTX))
        );
    }
    assert_eq!(
        get_x(CTX, ResponseTemplate::new(500)).await,
        Err(ProviderError::BadResponse(CTX, "HTTP 500".into()))
    );
    let limited = ResponseTemplate::new(429).insert_header("retry-after", "30");
    let rate = get_x(CTX, limited).await.unwrap_err();
    assert_eq!(
        rate.to_string(),
        "Cursor is rate limiting requests. Retrying later."
    );
    assert_eq!(rate.usage_error().retry_after().unwrap().as_secs(), 30);
}

/// Grokbot's 403 is a transient custom error; challenges win over status.
#[tokio::test]
async fn test_status_mapping_for_grokbot_and_challenges() {
    let g = ProviderErrorContext::Grokbot;
    let forbidden = get_x(g, ResponseTemplate::new(403).set_body_string("{}"))
        .await
        .unwrap_err();
    assert_eq!(
        forbidden.to_string(),
        "Grok Bot is not available on this Cursor account."
    );
    assert!(matches!(
        forbidden.usage_error(),
        UsageError::BadResponse(_)
    ));
    let html = ResponseTemplate::new(403).set_body_string("<html>blocked</html>");
    let blocked = get_x(g, html).await.unwrap_err();
    assert!(
        blocked
            .to_string()
            .contains("bot-protection check (HTTP 403)")
    );
    let mitigated = ResponseTemplate::new(200).insert_header("cf-mitigated", "challenge");
    assert!(get_x(g, mitigated).await.is_err());
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
    let http = http_for(ProviderErrorContext::Grokbot, &server.uri());
    let cookie = "WorkosCursorSessionToken=abc";
    let body = http.get("/ok", cookie, "https://cursor.com/bot").await;
    assert_eq!(body.unwrap(), b"{}");
    let denied = http.get("/denied", cookie, "https://cursor.com/bot").await;
    assert_eq!(
        denied,
        Err(ProviderError::Unauthorized(ProviderErrorContext::Grokbot))
    );
}

/// A refused connection maps to a network error without leaking the URL.
#[tokio::test]
async fn test_http_transport_failure_is_network_error() {
    let http = http_for(CTX, "http://127.0.0.1:1");
    let e = http
        .get("/x", "c=1", "https://cursor.com")
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
