//! Cancellation and Set-Cookie handling of the networking layer
//! (FEAT-core-net-* rows, plus `testCancelledRequestThrowsCancellationError`).
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use reqwest::header::{HeaderMap, HeaderValue};
use tokenmon_core::net::{
    ProviderHttp, RequestOptions, set_cookie_headers, split_folded_set_cookie, with_cancellation,
};
use tokenmon_core::{ManualClock, ProviderError, ProviderErrorContext as Ctx};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Repeated `Set-Cookie` headers are collected one entry per header line.
#[test]
fn test_set_cookie_headers_collects_each_line() {
    let mut h = HeaderMap::new();
    h.append("set-cookie", HeaderValue::from_static("a=1"));
    h.append("Set-Cookie", HeaderValue::from_static("b=2; Secure"));
    assert_eq!(set_cookie_headers(&h), ["a=1", "b=2; Secure"]);
}

/// Comma-folded values split back into cookies; `Expires` commas are kept.
#[test]
fn test_split_folded_set_cookie_keeps_expires_commas() {
    assert_eq!(
        split_folded_set_cookie("a=1, b=2; Secure,c=3"),
        ["a=1", "b=2; Secure", "c=3"]
    );
    let dated = "a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/, b=2";
    assert_eq!(
        split_folded_set_cookie(dated),
        ["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/", "b=2"]
    );
    assert_eq!(split_folded_set_cookie(" "), Vec::<String>::new());
}

/// A cancelled in-flight request is `Cancelled` (skipped by pollers), not a
/// network error.
#[tokio::test]
async fn test_cancelled_request_is_cancelled_not_network_error() {
    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;
    let clock = Arc::new(ManualClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ));
    let c = ProviderHttp::new(Ctx::Claude, None, clock).unwrap();
    let uri = server.uri();
    let request = c.get(&uri, "slow", RequestOptions::default());
    let cancel = tokio::time::sleep(Duration::from_millis(50));
    let e = with_cancellation(cancel, request).await.unwrap_err();
    assert_eq!(e, ProviderError::Cancelled);
    assert!(e.is_cancelled());
    assert!(!ProviderError::Unauthorized(Ctx::Claude).is_cancelled());
}
