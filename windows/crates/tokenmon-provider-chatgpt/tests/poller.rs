//! Port of the ChatGPT cases of `PollerSessionGuardTests.swift`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Utc};
use common::FakeSession;
use tokenmon_core::{
    AuthEvent, AuthEventListener, Clock, ManualClock, ProviderError, ProviderErrorContext as C,
};
use tokenmon_provider_chatgpt::{
    AccessTokenCache, ChatGptPoller, Fetch, FetchFn, RefreshOutcome, parse_usage,
};

const COOKIE: &str = "__Secure-next-auth.session-token=live";

fn fetch_ok(cookies: Vec<String>) -> Fetch {
    Fetch {
        response: parse_usage(common::USAGE_BODY.as_bytes()).unwrap(),
        fetched_at: Utc.timestamp_opt(1_790_000_000, 0).unwrap(),
        set_cookie_headers: cookies,
    }
}

fn poller(session: &Arc<FakeSession>, f: Arc<FetchFn>) -> ChatGptPoller {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::from_unix(1_790_000_000));
    ChatGptPoller::new(session.clone(), clock, Arc::new(AccessTokenCache::new()), f)
}

fn scripted(
    f: impl Fn(usize) -> Result<Fetch, ProviderError> + Send + Sync + 'static,
) -> Arc<FetchFn> {
    let n = Arc::new(AtomicUsize::new(0));
    Arc::new(move |_cookie, _gen| {
        let attempt = n.fetch_add(1, Ordering::SeqCst) + 1;
        let r = f(attempt);
        Box::pin(async move { r })
    })
}

/// `testChatGPTStaleUnauthorizedDoesNotInvalidateNewSession`: sign-out and a new
/// session during the fetch, then a 401: the new session stays signed in.
#[tokio::test]
async fn test_stale_unauthorized_does_not_invalidate_new_session() {
    let session = FakeSession::signed_in("__Secure-next-auth.session-token=old");
    let s2 = session.clone();
    let f: Arc<FetchFn> = Arc::new(move |_, _| {
        s2.sign_out();
        s2.save("__Secure-next-auth.session-token=new");
        Box::pin(async { Err(ProviderError::Unauthorized(C::ChatGpt)) })
    });
    let p = poller(&session, f);
    assert_eq!(p.refresh_now().await, RefreshOutcome::Skipped);
    let state = tokenmon_core::AuthSession::state(&*session);
    assert!(state.is_signed_in && !state.needs_sign_in);
    assert_eq!(state.consecutive_auth_failures, 0);
    assert!(p.last_error().is_none());
}

/// `testChatGPTCurrentUnauthorizedInvalidatesSession`: three strikes.
#[tokio::test]
async fn test_current_unauthorized_invalidates_session() {
    let session = FakeSession::signed_in(COOKIE);
    let p = poller(
        &session,
        scripted(|_| Err(ProviderError::Unauthorized(C::ChatGpt))),
    );
    for _ in 0..3 {
        p.refresh_now().await;
    }
    let state = tokenmon_core::AuthSession::state(&*session);
    assert!(state.needs_sign_in && !state.is_signed_in);
}

/// `testChatGPTTransientUnauthorizedRecoversOnNextPoll`.
#[tokio::test]
async fn test_transient_unauthorized_recovers_on_next_poll() {
    let session = FakeSession::signed_in(COOKIE);
    let p = poller(
        &session,
        scripted(|n| {
            if n == 1 {
                Err(ProviderError::Unauthorized(C::ChatGpt))
            } else {
                Ok(fetch_ok(vec![]))
            }
        }),
    );
    p.refresh_now().await;
    let state = tokenmon_core::AuthSession::state(&*session);
    assert!(state.is_signed_in);
    assert_eq!(state.consecutive_auth_failures, 1);
    assert_eq!(p.refresh_now().await, RefreshOutcome::Success);
    let state = tokenmon_core::AuthSession::state(&*session);
    assert_eq!(state.consecutive_auth_failures, 0);
    assert!(state.is_signed_in && !state.needs_sign_in);
    assert!(p.snapshot().is_some());
}

/// `testChatGPTFailureAfterDataReportsError`: snapshot kept, error set, still
/// signed in, then cleared by the next success.
#[tokio::test]
async fn test_failure_after_data_reports_error() {
    let session = FakeSession::signed_in(COOKIE);
    let p = poller(
        &session,
        scripted(|n| {
            if n == 2 {
                Err(ProviderError::Network(C::ChatGpt, "offline".into()))
            } else {
                Ok(fetch_ok(vec![]))
            }
        }),
    );
    p.refresh_now().await;
    assert!(p.last_error().is_none());
    assert!(matches!(p.refresh_now().await, RefreshOutcome::Failed(_)));
    assert!(p.snapshot().is_some());
    assert!(p.last_error().is_some());
    assert!(!tokenmon_core::AuthSession::state(&*session).needs_sign_in);
    p.refresh_now().await;
    assert!(p.last_error().is_none());
    assert!(p.last_refreshed_at().is_some());
}

/// `testChatGPTRefreshedCookieIsPersisted`: the renewed session cookie is
/// stored; the `_ga` tracker is not.
#[tokio::test]
async fn test_refreshed_cookie_is_persisted() {
    let session = FakeSession::signed_in("__Secure-next-auth.session-token=old");
    let p = poller(
        &session,
        scripted(|_| {
            Ok(fetch_ok(vec![
                "__Secure-next-auth.session-token=renewed; Path=/; Secure; HttpOnly".into(),
                "_ga=tracker; Path=/".into(),
            ]))
        }),
    );
    p.refresh_now().await;
    assert_eq!(
        tokenmon_core::AuthSession::cookie_header(&*session).as_deref(),
        Some("__Secure-next-auth.session-token=renewed")
    );
}

/// No cookie: skipped, sign-in prompt shown, `lastError` set only without a snapshot.
#[tokio::test]
async fn test_missing_cookie_skips_and_prompts_sign_in() {
    let session = Arc::new(FakeSession::default());
    let p = poller(
        &session,
        scripted(|_| unreachable!("no fetch without a cookie")),
    );
    assert_eq!(p.refresh_now().await, RefreshOutcome::Skipped);
    assert_eq!(
        p.last_error().unwrap().to_string(),
        "Sign in to ChatGPT to load usage."
    );
    assert!(tokenmon_core::AuthSession::state(&*session).needs_sign_in);
}

/// A disabled provider never fetches; headline accessors follow the snapshot;
/// auth events clear it.
#[tokio::test]
async fn test_disabled_headline_and_clear_on_auth_event() {
    let session = FakeSession::signed_in(COOKIE);
    let p = poller(&session, scripted(|_| Ok(fetch_ok(vec![]))));
    p.set_enabled(false);
    assert_eq!(p.refresh_now().await, RefreshOutcome::Skipped);
    assert!(p.snapshot().is_none());
    p.set_enabled(true);
    assert_eq!(p.refresh_now().await, RefreshOutcome::Success);
    assert_eq!(p.headline_used_percent(), Some(12.0));
    assert_eq!(p.headline_resets_at().unwrap().timestamp(), 1_787_300_000);
    p.on_auth_event(AuthEvent::SignedOut);
    assert!(p.snapshot().is_none() && p.last_refreshed_at().is_none());
}

/// `live_fetch` end to end over the fake transport: key is the generation, the
/// cache survives between polls, and a 401 on the cached token re-exchanges.
#[tokio::test]
async fn test_live_fetch_uses_generation_as_cache_key() {
    let session = FakeSession::signed_in(COOKIE);
    let transport = Arc::new(common::FakeTransport::new(common::jwt_exp(1_790_003_600)));
    let cache = Arc::new(AccessTokenCache::new());
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::from_unix(1_790_000_000));
    let f = ChatGptPoller::live_fetch(
        Arc::clone(&transport),
        Arc::clone(&cache),
        Arc::clone(&clock),
    );
    let p = ChatGptPoller::new(session.clone(), clock, cache, f);
    p.refresh_now().await;
    p.refresh_now().await;
    assert_eq!(transport.exchanges(), 1);
    session.save(COOKIE); // new generation: different cache key
    p.refresh_now().await;
    assert_eq!(transport.exchanges(), 2);
}
