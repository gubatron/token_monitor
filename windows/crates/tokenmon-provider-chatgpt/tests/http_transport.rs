//! The live transport against wiremock: headers, paths and `Set-Cookie` folding.
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]

mod common;

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tokenmon_core::net::ProviderHttp;
use tokenmon_core::{ProviderErrorContext as C, SystemClock};
use tokenmon_provider_chatgpt::{AccessTokenCache, ChatGptUsageClient, HttpTransport};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The two-step flow sends the cookie and Referer to the session endpoint, the
/// bearer token and account id to the usage endpoint, and returns both
/// `Set-Cookie` values in order.
#[tokio::test]
async fn test_exchange_and_usage_over_http() {
    let server = MockServer::start().await;
    let token = common::jwt(
        r#"{"exp":4102444800,"https://api.openai.com/auth":{"chatgpt_account_id":"acct_7"}}"#,
    );
    Mock::given(method("GET"))
        .and(path("/api/auth/session"))
        .and(header("cookie", "sess=1"))
        .and(header("referer", "https://chatgpt.com/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"accessToken":"{token}"}}"#))
                .append_header(
                    "set-cookie",
                    "__Secure-next-auth.session-token=renewed; Path=/",
                ),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/backend-api/wham/usage"))
        .and(header("authorization", format!("Bearer {token}").as_str()))
        .and(header("chatgpt-account-id", "acct_7"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(common::USAGE_BODY)
                .append_header("set-cookie", "_ga=t"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let http = ProviderHttp::new(C::ChatGpt, Some("test"), Arc::new(SystemClock)).unwrap();
    let transport = Arc::new(HttpTransport::with_base_url(http, &server.uri()));
    let client = ChatGptUsageClient::new(
        "sess=1",
        transport,
        Some(Arc::new(AccessTokenCache::new())),
        "1",
    );
    let fetch = client
        .fetch_usage(Utc.timestamp_opt(1_790_000_000, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(fetch.response.primary.unwrap().used_percent, 12.0);
    assert_eq!(
        fetch.set_cookie_headers,
        ["__Secure-next-auth.session-token=renewed; Path=/", "_ga=t"]
    );
}

/// A 401 from the session endpoint maps to the ChatGPT unauthorized wording.
#[tokio::test]
async fn test_session_401_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/api/auth/session"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let http = ProviderHttp::new(C::ChatGpt, None, Arc::new(SystemClock)).unwrap();
    let transport = Arc::new(HttpTransport::with_base_url(http, &server.uri()));
    let client = ChatGptUsageClient::new("sess=1", transport, None, "1");
    let err = client
        .fetch_usage(Utc.timestamp_opt(1_790_000_000, 0).unwrap())
        .await
        .err()
        .unwrap();
    assert_eq!(err.to_string(), "ChatGPT session expired. Sign in again.");
}
