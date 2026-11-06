//! The API key must never appear in Debug output or error strings.

use std::sync::Arc;

use chrono::Utc;
use tokenmon_core::{AuthSession, InMemoryCredentialStore, ManualClock};
use tokenmon_provider_openrouter::{
    ApiKey, Endpoint, EndpointFetcher, NoCostEstimator, OpenRouterAuthSession,
    OpenRouterUsageClient, ReqwestFetcher, parse_activity, parse_credits, parse_key,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "sk-or-v1-SUPERSECRETKEYVALUE0123456789";

fn assert_clean(text: &str) {
    assert!(!text.contains(SECRET), "leaked key in: {text}");
    assert!(
        !text.contains("SUPERSECRET"),
        "leaked key fragment in: {text}"
    );
}

/// Debug output of every type that can hold the key is redacted.
#[test]
fn test_debug_output_is_redacted() {
    let key = ApiKey::new(SECRET);
    assert_clean(&format!("{key:?} {key:#?}"));
    let store = Arc::new(InMemoryCredentialStore::default());
    let session = OpenRouterAuthSession::new(store);
    session.save_api_key(SECRET).unwrap();
    assert_clean(&format!("{session:?}"));
    let fetcher = ReqwestFetcher::new("https://openrouter.ai/api/v1", key.clone()).unwrap();
    assert_clean(&format!("{fetcher:?}"));
    assert_clean(&format!("{:?}", session.state()));
}

/// `redact` replaces every occurrence.
#[test]
fn test_redact_replaces_all_occurrences() {
    let key = ApiKey::new(SECRET);
    let out = key.redact(&format!("a {SECRET} b {SECRET}"));
    assert_eq!(out, "a [redacted] b [redacted]");
}

/// A server that reflects the key in error bodies or malformed JSON does not
/// get it into our errors.
#[tokio::test]
async fn test_reflected_key_never_reaches_error_strings() {
    let server = MockServer::start().await;
    let echo = format!("{{\"data\":{{\"usage\":\"{SECRET}\"}}}}");
    for (p, t) in [
        (
            "/key",
            ResponseTemplate::new(500).set_body_string(echo.clone()),
        ),
        (
            "/credits",
            ResponseTemplate::new(401).set_body_string(echo.clone()),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(t)
            .mount(&server)
            .await;
    }
    let f = ReqwestFetcher::new(&server.uri(), ApiKey::new(SECRET)).unwrap();
    for e in [Endpoint::Key, Endpoint::Credits] {
        let err = f.fetch(e).await.unwrap_err();
        assert_clean(&err.to_string());
        assert_clean(&format!("{err:?}"));
    }
    // Reflected in a 200 body that fails to decode.
    for err in [
        parse_key(echo.as_bytes()).unwrap_err(),
        parse_credits(echo.as_bytes()).unwrap_err(),
        parse_activity(echo.as_bytes()).unwrap_err(),
    ] {
        assert_clean(&err.to_string());
        assert_clean(&format!("{err:?}"));
    }
}

/// A transport failure (nothing listening) does not leak the key.
#[tokio::test]
async fn test_network_error_does_not_leak_key() {
    let f = ReqwestFetcher::new("http://127.0.0.1:1/", ApiKey::new(SECRET)).unwrap();
    let err = f.fetch(Endpoint::Key).await.unwrap_err();
    assert!(matches!(err, tokenmon_core::ProviderError::Network(..)));
    assert_clean(&err.to_string());
}

/// The end-to-end poller error surface stays clean too.
#[tokio::test]
async fn test_snapshot_error_through_client_is_clean() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!("[\"{SECRET}\"]")))
        .mount(&server)
        .await;
    let f = Arc::new(ReqwestFetcher::new(&server.uri(), ApiKey::new(SECRET)).unwrap());
    let client = OpenRouterUsageClient::new(f, Arc::new(NoCostEstimator));
    let err = client.fetch_snapshot(Utc::now()).await.unwrap_err();
    assert_clean(&err.to_string());
    let _ = ManualClock::from_unix(0);
}
