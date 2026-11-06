//! Client tests over a fake transport (stubbed HTTP by path; unknown paths
//! answer 404) plus one end-to-end check of the reqwest transport against a
//! local mock server.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{FIXTURE_A_HEX, clock_at, hex, utc};
use tokenmon_core::{BoxFuture, ProviderError, UsageError};
use tokenmon_provider_grok::client::{
    BILLING_ENDPOINT, GrokEndpoints, GrokUsageClient, HttpMethod, HttpRequest, HttpResponse,
    HttpTransport, REST_CANDIDATES, ReqwestTransport, is_bot_challenge,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct FakeTransport {
    routes: Mutex<HashMap<String, Result<HttpResponse, String>>>,
    requests: Mutex<Vec<HttpRequest>>,
    hang: bool,
}

impl FakeTransport {
    fn route(&self, url: &str, response: HttpResponse) {
        self.routes
            .lock()
            .unwrap()
            .insert(url.to_owned(), Ok(response));
    }

    fn fail(&self, url: &str, reason: &str) {
        self.routes
            .lock()
            .unwrap()
            .insert(url.to_owned(), Err(reason.to_owned()));
    }

    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.url.clone())
            .collect()
    }
}

impl HttpTransport for FakeTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, String>> {
        Box::pin(async move {
            if self.hang {
                std::future::pending::<()>().await;
            }
            let url = request.url.clone();
            self.requests.lock().unwrap().push(request);
            self.routes.lock().unwrap().get(&url).cloned().unwrap_or(Ok(
                HttpResponse {
                    status: 404,
                    ..HttpResponse::default()
                },
            ))
        })
    }
}

fn resp(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: body.to_vec(),
    }
}

fn client(transport: &Arc<FakeTransport>, cookie: Option<&str>) -> GrokUsageClient {
    GrokUsageClient::new(
        transport.clone(),
        clock_at(utc(2026, 7, 15, 12, 0)),
        "TokenMon/test",
        cookie.map(str::to_owned),
        Some("me@example.com".to_owned()),
    )
}

fn is_unauthorized(e: &ProviderError) -> bool {
    matches!(e.usage_error(), UsageError::Unauthorized)
}

/// Billing 200 with fixture A gives used 36; no REST probe is requested.
#[tokio::test]
async fn test_billing_success_skips_rest_probes() {
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(200, &hex(FIXTURE_A_HEX)));
    let snapshot = client(&t, Some("sso=a")).fetch_usage().await.unwrap();
    assert!((snapshot.used_percent - 36.0).abs() < 0.01);
    assert_eq!(snapshot.remaining_percent, 100.0 - snapshot.used_percent);
    assert_eq!(snapshot.account_email.as_deref(), Some("me@example.com"));
    assert_eq!(snapshot.products.len(), 2);
    assert_eq!(t.paths(), [BILLING_ENDPOINT]);
}

/// The billing request carries exactly the documented headers and body.
#[tokio::test]
async fn test_billing_request_shape() {
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(200, &hex(FIXTURE_A_HEX)));
    client(&t, Some("sso=a")).fetch_usage().await.unwrap();
    let request = t.requests.lock().unwrap()[0].clone();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.body, [0, 0, 0, 0, 0]);
    assert_eq!(request.timeout, Duration::from_secs(15));
    for (name, value) in [
        ("Cookie", "sso=a"),
        ("Content-Type", "application/grpc-web+proto"),
        ("x-grpc-web", "1"),
        ("x-user-agent", "connect-es/2.1.1"),
        ("Accept", "*/*"),
        ("Origin", "https://grok.com"),
        ("Referer", "https://grok.com/?_s=usage"),
        ("User-Agent", "TokenMon/test"),
    ] {
        assert_eq!(request.header(name), Some(value), "{name}");
    }
}

/// No cookie header is `NotSignedIn` before any request.
#[tokio::test]
async fn test_not_signed_in_makes_no_request() {
    let t = Arc::new(FakeTransport::default());
    let err = client(&t, None).fetch_usage().await.unwrap_err();
    assert!(matches!(err, ProviderError::NotSignedIn(_)));
    assert!(t.paths().is_empty());
}

/// Billing 401 is unauthorized without probing.
#[tokio::test]
async fn test_billing_unauthorized_rejects_without_probing() {
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(401, b""));
    let err = client(&t, Some("sso=a")).fetch_usage().await.unwrap_err();
    assert!(is_unauthorized(&err));
    assert_eq!(t.paths(), [BILLING_ENDPOINT]);
}

/// A trailers-only `grpc-status: 16` reply (empty body) is unauthorized.
#[tokio::test]
async fn test_trailers_only_unauthenticated_is_unauthorized() {
    let t = Arc::new(FakeTransport::default());
    t.route(
        BILLING_ENDPOINT,
        HttpResponse {
            status: 200,
            headers: vec![
                ("grpc-status".into(), "16".into()),
                ("Grpc-Message".into(), "unauthenticated".into()),
            ],
            body: Vec::new(),
        },
    );
    let err = client(&t, Some("sso=a")).fetch_usage().await.unwrap_err();
    assert!(is_unauthorized(&err));
    assert_eq!(t.paths().len(), 1);
}

/// A body trailer with status 16 is unauthorized too.
#[tokio::test]
async fn test_body_trailer_unauthenticated_is_unauthorized() {
    let text = b"grpc-status:16\r\n";
    let mut body = vec![0x80];
    body.extend((text.len() as u32).to_be_bytes());
    body.extend(text);
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(200, &body));
    let err = client(&t, Some("sso=a")).fetch_usage().await.unwrap_err();
    assert!(is_unauthorized(&err));
}

/// A 403 HTML challenge is transient, not unauthorized; the probes run.
#[tokio::test]
async fn test_billing_challenge_is_transient() {
    let t = Arc::new(FakeTransport::default());
    t.route(
        BILLING_ENDPOINT,
        resp(403, b"  <html>Just a moment...</html>"),
    );
    let err = client(&t, Some("sso=a")).fetch_usage().await.unwrap_err();
    assert!(!is_unauthorized(&err));
    assert!(err.to_string().contains("bot-protection"));
    assert_eq!(t.paths().len(), 1 + REST_CANDIDATES.len());
    assert!(is_bot_challenge(&HttpResponse {
        status: 200,
        headers: vec![("CF-Mitigated".into(), "challenge".into())],
        body: Vec::new()
    }));
}

/// Billing 500 and every REST probe 401: not unauthorized; billing then all
/// four REST paths were requested in order.
#[tokio::test]
async fn test_rest_probe_rejection_never_invalidates() {
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(500, b"boom"));
    for url in REST_CANDIDATES {
        t.route(url, resp(401, b""));
    }
    let err = client(&t, Some("sso=a")).fetch_usage().await.unwrap_err();
    assert!(!is_unauthorized(&err));
    assert_eq!(err.to_string(), "Usage request failed (HTTP 500).");
    let mut expected = vec![BILLING_ENDPOINT.to_owned()];
    expected.extend(REST_CANDIDATES.map(str::to_owned));
    assert_eq!(t.paths(), expected);
}

/// Billing 503 and REST #3 returning usage supplies the snapshot.
#[tokio::test]
async fn test_rest_probe_supplies_usage_when_billing_fails() {
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(503, b""));
    t.route(
        "https://grok.com/rest/billing/usage",
        resp(200, br#"{"usage":{"usedPercent":42}}"#),
    );
    let snapshot = client(&t, Some("sso=a")).fetch_usage().await.unwrap();
    assert_eq!(snapshot.used_percent, 42.0);
    assert_eq!(snapshot.account_email.as_deref(), Some("me@example.com"));
    assert_eq!(t.paths().len(), 4);
    let probe = t.requests.lock().unwrap()[3].clone();
    assert_eq!(probe.method, HttpMethod::Get);
    assert_eq!(probe.header("Accept"), Some("application/json"));
}

/// A transport failure on billing is a Network error, then REST is probed;
/// the original error is returned when nothing succeeds.
#[tokio::test]
async fn test_transport_failure_reports_network_error() {
    let t = Arc::new(FakeTransport::default());
    t.fail(BILLING_ENDPOINT, "offline");
    let err = client(&t, Some("sso=a")).fetch_usage().await.unwrap_err();
    assert_eq!(err.to_string(), "Grok network error: offline");
}

/// An empty 200 REST body or a non-200 REST status does not count.
#[tokio::test]
async fn test_rest_probe_requires_200_and_body() {
    let t = Arc::new(FakeTransport::default());
    t.route(BILLING_ENDPOINT, resp(500, b""));
    t.route(REST_CANDIDATES[0], resp(200, b""));
    t.route(REST_CANDIDATES[1], resp(201, br#"{"usedPercent":9}"#));
    assert!(client(&t, Some("sso=a")).fetch_usage().await.is_err());
}

/// Cancellation is future drop: a never-responding request can be abandoned
/// without recording anything (Rust equivalent of `CancellationError`).
#[tokio::test(start_paused = true)]
async fn test_cancelled_fetch_is_dropped_future() {
    let t = Arc::new(FakeTransport {
        hang: true,
        ..FakeTransport::default()
    });
    let c = client(&t, Some("sso=a"));
    let result = tokio::time::timeout(Duration::from_secs(5), c.fetch_usage()).await;
    assert!(result.is_err(), "the fetch is still pending when dropped");
    assert!(t.paths().is_empty());
}

/// The reqwest transport sends the billing call to a local mock server and
/// parses the fixture end to end.
#[tokio::test]
async fn test_reqwest_transport_against_mock_server() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/billing"))
        .and(header("x-grpc-web", "1"))
        .and(header("cookie", "sso=a"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/grpc-web+proto")
                .set_body_bytes(hex(FIXTURE_A_HEX)),
        )
        .mount(&server)
        .await;
    let transport = Arc::new(ReqwestTransport::new().unwrap());
    let c = GrokUsageClient::new(
        transport,
        clock_at(utc(2026, 7, 15, 12, 0)),
        "TokenMon/test",
        Some("sso=a".to_owned()),
        None,
    )
    .with_endpoints(GrokEndpoints {
        billing: format!("{}/billing", server.uri()),
        rest: Vec::new(),
    });
    let snapshot = c.fetch_usage().await.unwrap();
    assert!((snapshot.used_percent - 36.0).abs() < 0.01);
}

/// An unreachable host is a network error carrying no URL.
#[tokio::test]
async fn test_reqwest_transport_connection_refused() {
    let transport = ReqwestTransport::new().unwrap();
    let err = transport
        .send(HttpRequest {
            method: HttpMethod::Get,
            url: "http://127.0.0.1:1/".to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout: Duration::from_secs(2),
        })
        .await
        .unwrap_err();
    assert!(!err.contains("127.0.0.1"));
}
