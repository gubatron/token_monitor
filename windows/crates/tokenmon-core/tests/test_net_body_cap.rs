//! `ProviderHttp` response-size cap (divergence DC1 follow-up): Content-Length
//! rejection, incremental rejection of chunked bodies, per-request override.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use tokenmon_core::net::{DEFAULT_MAX_RESPONSE_BYTES, ProviderHttp, RequestOptions, ensure_room};
use tokenmon_core::{ManualClock, ProviderError, ProviderErrorContext as Ctx};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http() -> ProviderHttp {
    ProviderHttp::new(
        Ctx::Cursor,
        Some("test"),
        Arc::new(ManualClock::from_unix(0)),
    )
    .unwrap()
}

fn is_too_large(e: &ProviderError) -> bool {
    matches!(e, ProviderError::BadResponse(_, m) if m == "Response too large")
}

/// The default cap is 16 MiB.
#[test]
fn test_default_cap_is_16_mib() {
    assert_eq!(DEFAULT_MAX_RESPONSE_BYTES, 16 * 1024 * 1024);
}

/// `ensure_room` accepts exactly the limit and rejects one more byte, even at
/// `usize::MAX`.
#[test]
fn test_ensure_room_boundaries() {
    assert!(ensure_room(0, 10, 10).is_ok());
    assert!(ensure_room(5, 6, 10).is_err());
    assert!(ensure_room(usize::MAX, 1, 10).is_err());
}

/// A body within the limit is returned whole.
#[tokio::test]
async fn test_body_at_limit_is_accepted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 100]))
        .mount(&server)
        .await;
    let body = http()
        .with_max_response_bytes(100)
        .get(&server.uri(), "x", RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(body.len(), 100);
}

/// A declared Content-Length over the cap fails with the typed message.
#[tokio::test]
async fn test_content_length_over_limit_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 101]))
        .mount(&server)
        .await;
    let err = http()
        .with_max_response_bytes(100)
        .get(&server.uri(), "x", RequestOptions::default())
        .await
        .unwrap_err();
    assert!(is_too_large(&err), "{err:?}");
}

/// The per-request override beats the client default in both directions.
#[tokio::test]
async fn test_per_request_override_wins() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 500]))
        .mount(&server)
        .await;
    let client = http().with_max_response_bytes(100);
    let ok = client
        .get(
            &server.uri(),
            "x",
            RequestOptions {
                max_response_bytes: Some(1000),
                ..RequestOptions::default()
            },
        )
        .await;
    assert_eq!(ok.unwrap().len(), 500);
    let tight = http()
        .get(
            &server.uri(),
            "x",
            RequestOptions {
                max_response_bytes: Some(10),
                ..RequestOptions::default()
            },
        )
        .await;
    assert!(is_too_large(&tight.unwrap_err()));
}

/// A chunked body (no Content-Length) is cut off once it passes the cap.
#[tokio::test]
async fn test_chunked_body_over_limit_is_rejected_incrementally() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = sock.read(&mut buf).await;
        let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\n\r\n";
        let _ = sock.write_all(head.as_bytes()).await;
        // Far more than the limit; the client must hang up first.
        for _ in 0..1000 {
            let chunk = format!("100\r\n{}\r\n", "a".repeat(256));
            if sock.write_all(chunk.as_bytes()).await.is_err() {
                return;
            }
        }
        let _ = sock.write_all(b"0\r\n\r\n").await;
    });
    let err = http()
        .with_max_response_bytes(1024)
        .get(&format!("http://{addr}"), "x", RequestOptions::default())
        .await
        .unwrap_err();
    assert!(is_too_large(&err), "{err:?}");
}
