//! HTTP download and feed fetching against a local mock server.
#![allow(clippy::unwrap_used)]

use tokenmon_core::update::{
    InstallError, ReleaseFeedFetcher, ReleaseSource, UpdateError, sha256_hex,
};
use tokenmon_platform::install::{Downloader, HttpDownloader, HttpFeedFetcher, TrustPolicy};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn dl() -> HttpDownloader {
    HttpDownloader::new(TrustPolicy::AnyHostForTests, "TokenMon/test").unwrap()
}

/// The body is written and its SHA-256 returned.
#[tokio::test]
async fn test_download_returns_sha256_of_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a.zip"))
        .and(header("user-agent", "TokenMon/test"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello".to_vec()))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.zip");
    let got = dl()
        .download(&format!("{}/a.zip", server.uri()), &dest, 1024)
        .await
        .unwrap();
    assert_eq!(got.size, 5);
    assert_eq!(got.sha256, sha256_hex(b"hello"));
    assert_eq!(std::fs::read(&dest).unwrap(), b"hello");
}

/// A body over the cap is refused and no partial file stays.
#[tokio::test]
async fn test_download_stops_at_size_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 2048]))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.zip");
    let e = dl().download(&server.uri(), &dest, 1024).await.unwrap_err();
    assert_eq!(
        e,
        InstallError::Download("The release file is unexpectedly large.".into())
    );
    assert!(!dest.exists());
}

/// Non-2xx statuses are "not available".
#[tokio::test]
async fn test_download_404_is_not_available() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let e = dl()
        .download(&server.uri(), &dir.path().join("x"), 1024)
        .await
        .unwrap_err();
    assert_eq!(
        e,
        InstallError::Download("The release file was not available.".into())
    );
}

/// With the repository policy a non-GitHub final host is refused.
#[tokio::test]
async fn test_download_final_host_must_be_trusted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".to_vec()))
        .mount(&server)
        .await;
    let d = HttpDownloader::new(
        TrustPolicy::Repository(ReleaseSource::default()),
        "TokenMon/test",
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("x");
    let e = d.download(&server.uri(), &dest, 1024).await.unwrap_err();
    assert_eq!(
        e,
        InstallError::Download("The release file is not hosted on GitHub.".into())
    );
    assert!(!dest.exists());
}

/// The repository policy refuses to follow a redirect to an untrusted host.
#[tokio::test]
async fn test_download_does_not_follow_untrusted_redirect() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/final", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let d = HttpDownloader::new(
        TrustPolicy::Repository(ReleaseSource::default()),
        "TokenMon/test",
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    assert!(
        d.download(
            &format!("{}/start", server.uri()),
            &dir.path().join("x"),
            1024
        )
        .await
        .is_err()
    );
}

/// The feed request carries the GitHub headers and returns the body.
#[tokio::test]
async fn test_feed_fetch_sends_required_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("user-agent", "TokenMon/1.0"))
        .and(header("accept", "application/vnd.github+json"))
        .and(header("x-github-api-version", "2022-11-28"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;
    let f = HttpFeedFetcher::new(server.uri(), "TokenMon/1.0").unwrap();
    assert_eq!(f.fetch().await.unwrap(), b"[]");
}

/// 403 maps to the rate-limit message, other statuses to their code.
#[tokio::test]
async fn test_feed_fetch_status_mapping() {
    for (status, expected) in [
        (403u16, UpdateError::RateLimited),
        (500, UpdateError::BadResponse("HTTP 500".into())),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let f = HttpFeedFetcher::new(server.uri(), "TokenMon/1.0").unwrap();
        assert_eq!(f.fetch().await.unwrap_err(), expected);
    }
}

/// An oversized feed body is refused.
#[tokio::test]
async fn test_feed_fetch_caps_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
            b' ';
            tokenmon_core::update::MAX_FEED_BYTES
                + 1
        ]))
        .mount(&server)
        .await;
    let f = HttpFeedFetcher::new(server.uri(), "TokenMon/1.0").unwrap();
    assert!(matches!(f.fetch().await, Err(UpdateError::BadResponse(_))));
}
