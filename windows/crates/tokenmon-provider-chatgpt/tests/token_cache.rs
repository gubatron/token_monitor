//! Port of `ChatGPTAccessTokenCacheTests.swift` (now = 1_790_000_000).
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]

mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use common::{FakeTransport, jwt_exp};
use tokenmon_core::{ProviderError, ProviderErrorContext as C};
use tokenmon_provider_chatgpt::{AccessTokenCache, ChatGptUsageClient, RawReply, valid_until};

const T: i64 = 1_790_000_000;

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).unwrap()
}

fn client(
    transport: &Arc<FakeTransport>,
    cache: &Arc<AccessTokenCache>,
    cookie: &str,
    key: &str,
) -> ChatGptUsageClient<FakeTransport> {
    ChatGptUsageClient::new(cookie, Arc::clone(transport), Some(Arc::clone(cache)), key)
}

/// `testTokenIsReusedUntilRenewDeadline`: 1 exchange at t and t+60, a second at t+1801.
#[tokio::test]
async fn test_token_is_reused_until_renew_deadline() {
    let transport = Arc::new(FakeTransport::new(jwt_exp(T + 3600)));
    let cache = Arc::new(AccessTokenCache::new());
    let c = client(&transport, &cache, "c", "1");
    c.fetch_usage(at(T)).await.unwrap();
    c.fetch_usage(at(T + 60)).await.unwrap();
    assert_eq!(transport.exchanges(), 1);
    c.fetch_usage(at(T + 1801)).await.unwrap();
    assert_eq!(transport.exchanges(), 2);
    assert_eq!(transport.usages(), 3);
}

/// `testSessionCookiesAreReturnedOnlyWithAnExchange`.
#[tokio::test]
async fn test_session_cookies_are_returned_only_with_an_exchange() {
    let transport = Arc::new(FakeTransport::new(jwt_exp(T + 3600)));
    let cache = Arc::new(AccessTokenCache::new());
    let c = client(&transport, &cache, "c", "1");
    assert_eq!(
        c.fetch_usage(at(T)).await.unwrap().set_cookie_headers,
        ["session=renewed"]
    );
    assert_eq!(
        c.fetch_usage(at(T + 60)).await.unwrap().set_cookie_headers,
        Vec::<String>::new()
    );
}

/// `testRejectedCachedTokenIsReexchangedOnce`: the second usage call is a 401.
#[tokio::test]
async fn test_rejected_cached_token_is_reexchanged_once() {
    let transport = Arc::new(FakeTransport::scripted(jwt_exp(T + 3600), |n| {
        if n == 2 {
            Err(ProviderError::Unauthorized(C::ChatGpt))
        } else {
            Ok(RawReply {
                body: common::USAGE_BODY.into(),
                set_cookies: vec![],
            })
        }
    }));
    let cache = Arc::new(AccessTokenCache::new());
    let c = client(&transport, &cache, "c", "1");
    c.fetch_usage(at(T)).await.unwrap();
    let second = c.fetch_usage(at(T + 60)).await.unwrap();
    assert_eq!(second.response.primary.unwrap().used_percent, 12.0);
    assert_eq!(transport.exchanges(), 2);
}

/// A non-auth error from a cached token propagates without a re-exchange.
#[tokio::test]
async fn test_cached_token_network_error_propagates() {
    let transport = Arc::new(FakeTransport::scripted(jwt_exp(T + 3600), |n| {
        if n == 2 {
            Err(ProviderError::Network(C::ChatGpt, "offline".into()))
        } else {
            Ok(RawReply {
                body: common::USAGE_BODY.into(),
                set_cookies: vec![],
            })
        }
    }));
    let cache = Arc::new(AccessTokenCache::new());
    let c = client(&transport, &cache, "c", "1");
    c.fetch_usage(at(T)).await.unwrap();
    assert!(c.fetch_usage(at(T + 60)).await.is_err());
    assert_eq!(transport.exchanges(), 1);
}

/// `testValidUntilHonoursTokenExpiryAndRenewInterval`.
#[test]
fn test_valid_until_honours_token_expiry_and_renew_interval() {
    let now = at(T);
    assert_eq!(valid_until(&jwt_exp(T + 600), now), at(T + 300));
    assert_eq!(valid_until(&jwt_exp(T + 86_400), now), at(T + 1800));
    assert_eq!(valid_until("opaque", now), at(T + 300));
}

/// Extreme `exp` values never overflow and never panic (fuzz invariant).
#[test]
fn test_valid_until_is_total_for_extreme_exp() {
    let now = at(T);
    for exp in [
        "1e300",
        "-1e300",
        "9223372036854775807",
        "-9223372036854775808",
        "0",
    ] {
        let t = common::jwt(&format!(r#"{{"exp":{exp}}}"#));
        let v = valid_until(&t, now);
        assert!(v <= at(T + 1800));
    }
}

/// `testCacheIsScopedToKey`.
#[test]
fn test_cache_is_scoped_to_key() {
    let cache = AccessTokenCache::new();
    cache.store("tok", "1", at(T + 60));
    assert_eq!(cache.token("1", at(T)).as_deref(), Some("tok"));
    assert!(cache.token("2", at(T)).is_none());
    assert!(cache.token("1", at(T + 61)).is_none());
    cache.clear();
    assert!(cache.token("1", at(T)).is_none());
}

/// `testTokenIsReusedAfterCookieRenewal`: a new client with the renewed cookie
/// and the same key reuses the cached token.
#[tokio::test]
async fn test_token_is_reused_after_cookie_renewal() {
    let transport = Arc::new(FakeTransport::new(jwt_exp(T + 3600)));
    let cache = Arc::new(AccessTokenCache::new());
    client(&transport, &cache, "old", "1")
        .fetch_usage(at(T))
        .await
        .unwrap();
    client(&transport, &cache, "renewed", "1")
        .fetch_usage(at(T + 5))
        .await
        .unwrap();
    assert_eq!(transport.exchanges(), 1);
}

/// The account id from the JWT is forwarded to the usage call when present.
#[tokio::test]
async fn test_account_id_is_forwarded_to_usage() {
    let token = common::jwt(
        r#"{"exp":1790003600,"https://api.openai.com/auth":{"chatgpt_account_id":"acct_9"}}"#,
    );
    let transport = Arc::new(FakeTransport::new(token));
    let cache = Arc::new(AccessTokenCache::new());
    client(&transport, &cache, "c", "1")
        .fetch_usage(at(T))
        .await
        .unwrap();
    assert_eq!(
        transport.last_account.lock().unwrap().clone(),
        Some(Some("acct_9".to_owned()))
    );
}
