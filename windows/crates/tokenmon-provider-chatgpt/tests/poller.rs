//! Port of the ChatGPT cases of `PollerSessionGuardTests.swift`.
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Utc};
use common::FakeSession;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokenmon_core::polling::PollOutcome;
use tokenmon_core::{
    Clock, ManualClock, ProviderError, ProviderErrorContext as C, ProviderUsagePoller,
};
use tokenmon_provider_chatgpt::{
    AccessTokenCache, ChatGptPoller, ChatGptPollerConfig, Fetch, FetchFn, parse_usage,
};

fn failed() -> PollOutcome {
    PollOutcome::Failure { retry_after: None }
}

const COOKIE: &str = "__Secure-next-auth.session-token=live";

fn fetch_ok(cookies: Vec<String>) -> Fetch {
    Fetch {
        response: parse_usage(common::USAGE_BODY.as_bytes()).unwrap(),
        fetched_at: Utc.timestamp_opt(1_790_000_000, 0).unwrap(),
        set_cookie_headers: cookies,
    }
}

fn config(session: &Arc<FakeSession>, f: Arc<FetchFn>) -> ChatGptPollerConfig {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::from_unix(1_790_000_000));
    ChatGptPollerConfig {
        session: session.clone(),
        clock,
        token_cache: Arc::new(AccessTokenCache::new()),
        fetch: f,
        is_enabled: Arc::new(|| true),
        poll_interval: Arc::new(|_| None),
    }
}

fn poller(session: &Arc<FakeSession>, f: Arc<FetchFn>) -> Arc<ChatGptPoller> {
    ChatGptPoller::new(config(session, f))
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
    assert_eq!(p.perform_refresh().await, PollOutcome::Skipped);
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
        p.perform_refresh().await;
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
    p.perform_refresh().await;
    let state = tokenmon_core::AuthSession::state(&*session);
    assert!(state.is_signed_in);
    assert_eq!(state.consecutive_auth_failures, 1);
    assert_eq!(p.perform_refresh().await, PollOutcome::Success);
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
    p.perform_refresh().await;
    assert!(p.last_error().is_none());
    assert_eq!(p.perform_refresh().await, failed());
    assert!(p.snapshot().is_some());
    assert!(p.last_error().is_some());
    assert!(!tokenmon_core::AuthSession::state(&*session).needs_sign_in);
    p.perform_refresh().await;
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
    p.perform_refresh().await;
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
    assert_eq!(p.perform_refresh().await, PollOutcome::Skipped);
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
    let enabled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&enabled);
    let mut cfg = config(&session, scripted(|_| Ok(fetch_ok(vec![]))));
    cfg.is_enabled = Arc::new(move || flag.load(Ordering::SeqCst));
    let p = ChatGptPoller::new(cfg);
    assert_eq!(p.perform_refresh().await, PollOutcome::Skipped);
    assert!(p.snapshot().is_none());
    enabled.store(true, Ordering::SeqCst);
    assert_eq!(p.perform_refresh().await, PollOutcome::Success);
    assert_eq!(p.headline_used_percent(), Some(12.0));
    assert_eq!(p.headline_resets_at().unwrap().timestamp(), 1_787_300_000);
    p.clear_snapshot();
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
    let mut cfg = config(&session, f);
    cfg.clock = clock;
    cfg.token_cache = cache;
    let p = ChatGptPoller::new(cfg);
    p.perform_refresh().await;
    p.perform_refresh().await;
    assert_eq!(transport.exchanges(), 1);
    session.save(COOKIE); // new generation: different cache key
    p.perform_refresh().await;
    assert_eq!(transport.exchanges(), 2);
}

/// The poller drives itself on core's `PollingLoop`: a parked loop (interval
/// `None` while the menu is closed) refreshes at once when the menu opens, and
/// `set_menu_open` only wakes on a change.
#[tokio::test]
async fn test_set_menu_open_wakes_the_polling_loop() {
    let session = FakeSession::signed_in(COOKIE);
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fetches);
    let f = scripted(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(fetch_ok(vec![]))
    });
    let mut cfg = config(&session, f);
    cfg.poll_interval = Arc::new(|open| open.then_some(Duration::from_secs(60)));
    let p = ChatGptPoller::new(cfg);
    assert_eq!(p.provider(), tokenmon_core::ProviderId::ChatGpt);
    p.start();
    assert_eq!(p.polling_loop().delay_until_due(), None);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    p.set_menu_open(true);
    for _ in 0..200 {
        if fetches.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    p.stop();
}
