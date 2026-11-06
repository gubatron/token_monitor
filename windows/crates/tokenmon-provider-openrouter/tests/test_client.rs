//! Port of OpenRouterUsageClientTests.swift plus transport status mapping.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use tokenmon_core::{BoxFuture, ProviderError, ProviderErrorContext, UsageError};
use tokenmon_provider_openrouter::{
    ApiKey, BudgetSource, Endpoint, EndpointFetcher, NoCostEstimator, OpenRouterUsageClient,
    ReqwestFetcher,
};
use url::Url;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Canned per-endpoint bodies that record what was requested.
struct Canned {
    requested: Mutex<Vec<Endpoint>>,
    respond: Box<dyn Fn(Endpoint) -> Result<Vec<u8>, ProviderError> + Send + Sync>,
}

impl EndpointFetcher for Canned {
    fn fetch(&self, e: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>> {
        self.requested.lock().unwrap().push(e);
        let r = (self.respond)(e);
        Box::pin(async move { r })
    }
}

fn client(
    respond: impl Fn(Endpoint) -> Result<Vec<u8>, ProviderError> + Send + Sync + 'static,
) -> (Arc<Canned>, OpenRouterUsageClient) {
    let c = Arc::new(Canned {
        requested: Mutex::default(),
        respond: Box::new(respond),
    });
    (
        c.clone(),
        OpenRouterUsageClient::new(c, Arc::new(NoCostEstimator)),
    )
}

fn key_body(management: bool) -> Vec<u8> {
    format!(r#"{{"data":{{"usage":5,"limit":20,"is_management_key":{management}}}}}"#).into_bytes()
}

fn now() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

/// Endpoint URLs resolve under the API version path, with or without a
/// trailing slash on the base.
#[test]
fn test_endpoint_urls_resolve_under_api_version_path() {
    let base = Url::parse("https://openrouter.ai/api/v1").unwrap();
    let urls: Vec<_> = Endpoint::ALL
        .iter()
        .map(|e| e.url(&base).unwrap().to_string())
        .collect();
    assert_eq!(
        urls,
        [
            "https://openrouter.ai/api/v1/key",
            "https://openrouter.ai/api/v1/credits",
            "https://openrouter.ai/api/v1/activity"
        ]
    );
    let slashed = Url::parse("https://example.com/api/v1/").unwrap();
    assert_eq!(
        Endpoint::Key.url(&slashed).unwrap().as_str(),
        "https://example.com/api/v1/key"
    );
}

/// A non-management key never asks for the account endpoints.
#[tokio::test]
async fn test_non_management_key_skips_account_endpoints() {
    let (c, cl) = client(|_| Ok(key_body(false)));
    let s = cl.fetch_snapshot(now()).await.unwrap();
    assert_eq!(*c.requested.lock().unwrap(), [Endpoint::Key]);
    assert!(!s.is_management_key);
    assert_eq!(s.budget_source, Some(BudgetSource::KeyLimit));
}

/// A management key fetches all three and prefers account credits.
#[tokio::test]
async fn test_management_key_fetches_credits_and_activity() {
    let (c, cl) = client(|e| {
        Ok(match e {
            Endpoint::Key => key_body(true),
            Endpoint::Credits => br#"{"data":{"total_credits":40,"total_usage":10}}"#.to_vec(),
            Endpoint::Activity => br#"{"data":[]}"#.to_vec(),
        })
    });
    let s = cl.fetch_snapshot(now()).await.unwrap();
    let mut got = c.requested.lock().unwrap().clone();
    got.sort_by_key(|e| e.path());
    assert_eq!(got, [Endpoint::Activity, Endpoint::Credits, Endpoint::Key]);
    assert_eq!(s.budget_source, Some(BudgetSource::AccountCredits));
    assert_eq!(s.budget_usd, Some(40.0));
}

/// A key that does not state its flag is treated as possibly management.
#[tokio::test]
async fn test_absent_management_flag_fetches_account_endpoints() {
    let (c, cl) = client(|e| match e {
        Endpoint::Key => Ok(br#"{"data":{"usage":1}}"#.to_vec()),
        _ => Err(ProviderError::Unauthorized(
            ProviderErrorContext::OpenRouter,
        )),
    });
    cl.fetch_snapshot(now()).await.unwrap();
    assert_eq!(c.requested.lock().unwrap().len(), 3);
}

/// Failing or malformed account endpoints degrade to the key limit.
#[tokio::test]
async fn test_account_endpoint_failure_degrades_to_key_limit() {
    let (_, cl) = client(|e| match e {
        Endpoint::Key => Ok(key_body(true)),
        Endpoint::Credits => Err(ProviderError::Unauthorized(
            ProviderErrorContext::OpenRouter,
        )),
        Endpoint::Activity => Ok(b"<html>nope</html>".to_vec()),
    });
    let s = cl.fetch_snapshot(now()).await.unwrap();
    assert_eq!(s.budget_source, Some(BudgetSource::KeyLimit));
    assert!(s.models.is_empty());
}

/// `/key` failures propagate; malformed JSON is a bad-response error.
#[tokio::test]
async fn test_key_failures_propagate() {
    let (_, cl) = client(|_| {
        Err(ProviderError::Unauthorized(
            ProviderErrorContext::OpenRouter,
        ))
    });
    assert!(matches!(
        cl.fetch_snapshot(now()).await,
        Err(ProviderError::Unauthorized(_))
    ));
    let (_, cl) = client(|_| Ok(b"{}".to_vec()));
    assert!(matches!(
        cl.fetch_snapshot(now()).await,
        Err(ProviderError::BadResponse(..))
    ));
}

async fn fetcher(server: &MockServer) -> ReqwestFetcher {
    ReqwestFetcher::new(&server.uri(), ApiKey::new("sk-or-v1-wire")).unwrap()
}

/// The live transport sends bearer auth and JSON accept to `/key`.
#[tokio::test]
async fn test_transport_sends_bearer_and_returns_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .and(header("authorization", "Bearer sk-or-v1-wire"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"data\":{\"usage\":1}}"))
        .expect(1)
        .mount(&server)
        .await;
    let body = fetcher(&server).await.fetch(Endpoint::Key).await.unwrap();
    assert_eq!(body, b"{\"data\":{\"usage\":1}}");
}

async fn status_of(template: ResponseTemplate) -> ProviderError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .respond_with(template)
        .mount(&server)
        .await;
    fetcher(&server)
        .await
        .fetch(Endpoint::Key)
        .await
        .unwrap_err()
}

/// Status mapping: 401/403 unauthorized, 429 rate limited with Retry-After,
/// other statuses bad response, HTML 403 and `cf-mitigated` are bot challenges.
#[tokio::test]
async fn test_transport_status_mapping() {
    assert!(matches!(
        status_of(ResponseTemplate::new(401)).await,
        ProviderError::Unauthorized(_)
    ));
    assert!(matches!(
        status_of(ResponseTemplate::new(403).set_body_string("{}")).await,
        ProviderError::Unauthorized(_)
    ));
    let rl = status_of(ResponseTemplate::new(429).insert_header("retry-after", "7")).await;
    assert_eq!(
        rl.usage_error(),
        UsageError::RateLimited {
            retry_after: Some(7.0)
        }
    );
    assert_eq!(
        status_of(ResponseTemplate::new(503)).await,
        ProviderError::BadResponse(ProviderErrorContext::OpenRouter, "HTTP 503".into())
    );
    let bot = status_of(ResponseTemplate::new(403).set_body_string("<HTML>blocked</HTML>")).await;
    assert!(bot.to_string().contains("bot-protection"));
    let cf = status_of(ResponseTemplate::new(200).insert_header("cf-mitigated", "challenge")).await;
    assert!(cf.to_string().contains("bot-protection"));
}
