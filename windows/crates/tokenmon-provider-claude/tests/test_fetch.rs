//! The HTTP glue against a local mock server: request shape, parsing and the
//! error mapping the poller's auth strikes depend on.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tokenmon_core::net::ProviderHttp;
use tokenmon_core::{ProviderError, ProviderErrorContext, SystemClock, UsageError};
use tokenmon_provider_claude::ClaudeUsageClient;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const COOKIE: &str = "sessionKey=abc; lastActiveOrg=org-1";

fn client(server: &MockServer) -> ClaudeUsageClient {
    let http = ProviderHttp::new(
        ProviderErrorContext::Claude,
        Some("0.0.0"),
        Arc::new(SystemClock),
    )
    .unwrap();
    ClaudeUsageClient::new(http, server.uri())
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap()
}

/// The usage endpoint is hit for the cookie's org with the cookie and the
/// claude.ai referer; the body is parsed and stamped with the caller's `now`.
#[tokio::test]
async fn test_fetch_usage_sends_cookie_and_referer_and_parses_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/organizations/org-1/usage"))
        .and(header("cookie", COOKIE))
        .and(header("referer", "https://claude.ai/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"five_hour":{"utilization":12.5,"resets_at":"2026-10-14T20:00:00+00:00"},"seven_day":null}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let (response, stamp) = client(&server).fetch_usage(COOKIE, now()).await.unwrap();

    assert_eq!(response.five_hour.unwrap().used_percent, 12.5);
    assert!(response.seven_day.is_none());
    assert_eq!(stamp, now());
}

/// A session without an org id never reaches the network and counts as a
/// not-signed-in strike.
#[tokio::test]
async fn test_fetch_usage_without_org_makes_no_request() {
    let server = MockServer::start().await;
    let error = client(&server)
        .fetch_usage("sessionKey=abc", now())
        .await
        .unwrap_err();
    assert_eq!(error.usage_error(), UsageError::NotSignedIn);
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// HTTP 401 is `Unauthorized`.
#[tokio::test]
async fn test_fetch_usage_maps_401_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let error = client(&server)
        .fetch_usage(COOKIE, now())
        .await
        .unwrap_err();
    assert_eq!(error.usage_error(), UsageError::Unauthorized);
}

/// A 200 login page (an expired session redirected to sign-in) is
/// `Unauthorized`, not a parse failure.
#[tokio::test]
async fn test_fetch_usage_treats_a_login_page_as_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><form><input type=\"password\"></form></body></html>",
            ),
        )
        .mount(&server)
        .await;
    let error = client(&server)
        .fetch_usage(COOKIE, now())
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Unauthorized(_)));
}

/// 429 with `Retry-After` is `RateLimited` carrying the hint.
#[tokio::test]
async fn test_fetch_usage_maps_429_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "30"))
        .mount(&server)
        .await;
    let error = client(&server)
        .fetch_usage(COOKIE, now())
        .await
        .unwrap_err();
    assert_eq!(
        error.usage_error(),
        UsageError::RateLimited {
            retry_after: Some(30.0)
        }
    );
}

/// #50: a usage body past the provider's own cap is refused, far below the
/// shared 16 MiB default.
#[tokio::test]
async fn test_usage_body_over_the_claude_cap_is_refused() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
            b' ';
            tokenmon_provider_claude::fetch::MAX_RESPONSE_BYTES
                + 1
        ]))
        .mount(&server)
        .await;
    let err = client(&server)
        .fetch_usage(COOKIE, now())
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains("Response too large"), "{err:?}");
}
