//! #44: the provider client follows only https redirects to the provider's own
//! hosts, keeps `Set-Cookie` only from those hosts, and never lets a path move
//! the request to another origin. Hermetic: local mock servers.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tokenmon_core::net::{ProviderHttp, RequestOptions};
use tokenmon_core::{ManualClock, ProviderError, ProviderErrorContext as Ctx};
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    ))
}

fn same_host() -> ProviderHttp {
    ProviderHttp::new(Ctx::Claude, None, clock()).unwrap()
}

fn for_hosts(hosts: &[&str]) -> ProviderHttp {
    let hosts: Vec<String> = hosts.iter().map(|h| (*h).to_owned()).collect();
    ProviderHttp::for_hosts(Ctx::Claude, None, clock(), &hosts).unwrap()
}

/// `a` redirects to `b` under another host name (`localhost` vs `127.0.0.1`).
async fn redirecting_pair() -> (MockServer, MockServer, String) {
    let target = MockServer::start().await;
    Mock::given(path("/landed"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "sessionKey=evil; Path=/")
                .set_body_string("{}"),
        )
        .mount(&target)
        .await;
    let port = target.address().port();
    let location = format!("http://localhost:{port}/landed");
    let origin = MockServer::start().await;
    Mock::given(path("/start"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", location.as_str()))
        .mount(&origin)
        .await;
    (origin, target, location)
}

#[tokio::test]
async fn redirect_to_another_host_is_refused_by_default() {
    let (origin, target, _) = redirecting_pair().await;
    let e = same_host()
        .get(&origin.uri(), "/start", RequestOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Network(..)), "{e:?}");
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn redirect_to_a_listed_host_is_followed() {
    let (origin, target, _) = redirecting_pair().await;
    let r = for_hosts(&["127.0.0.1", "localhost"])
        .get_with_response(&origin.uri(), "/start", RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(r.status.as_u16(), 200);
    assert_eq!(target.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn redirect_to_an_unlisted_host_is_refused() {
    let (origin, target, _) = redirecting_pair().await;
    let e = for_hosts(&["127.0.0.1"])
        .get(&origin.uri(), "/start", RequestOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Network(..)), "{e:?}");
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn redirect_downgrade_to_plain_http_is_refused() {
    let origin = MockServer::start().await;
    Mock::given(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://example.com/landed"),
        )
        .mount(&origin)
        .await;
    let e = for_hosts(&["127.0.0.1", "example.com"])
        .get(&origin.uri(), "/start", RequestOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::Network(..)), "{e:?}");
}

#[tokio::test]
async fn set_cookie_is_dropped_when_the_responder_is_not_a_provider_host() {
    let server = MockServer::start().await;
    Mock::given(path("/x"))
        .respond_with(ResponseTemplate::new(200).insert_header("set-cookie", "sessionKey=v"))
        .mount(&server)
        .await;
    let kept = for_hosts(&["127.0.0.1"])
        .get_with_response(&server.uri(), "/x", RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(kept.set_cookie_headers(), ["sessionKey=v"]);
    let dropped = for_hosts(&["claude.ai"])
        .get_with_response(&server.uri(), "/x", RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(dropped.set_cookie_headers().len(), 0);
}

#[tokio::test]
async fn absolute_path_cannot_override_the_base_origin() {
    let server = MockServer::start().await;
    for bad in [
        "https://evil.example/x",
        "http://evil.example/x",
        "//evil.example/x",
    ] {
        let e = same_host()
            .get(&server.uri(), bad, RequestOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ProviderError::BadResponse(Ctx::Claude, format!("Invalid path {bad}"))
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn plain_http_to_a_public_host_is_refused_before_connecting() {
    let e = same_host()
        .get("http://example.com", "/x", RequestOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        e,
        ProviderError::BadResponse(Ctx::Claude, "Refusing a non-https URL".into())
    );
}
