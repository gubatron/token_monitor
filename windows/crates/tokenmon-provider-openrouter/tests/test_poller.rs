//! Poller state machine (spec 03 section 4.5).
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokenmon_core::{
    AuthSession, BoxFuture, InMemoryCredentialStore, ManualClock, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};
use tokenmon_provider_openrouter::{
    Endpoint, EndpointFetcher, NoCostEstimator, OpenRouterAuthSession, OpenRouterUsageClient,
    OpenRouterUsagePoller, RefreshOutcome,
};

struct Fake {
    calls: Arc<AtomicUsize>,
    unauthorized: Arc<AtomicBool>,
}

impl EndpointFetcher for Fake {
    fn fetch(&self, e: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>> {
        if e == Endpoint::Key {
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
        let r = if self.unauthorized.load(Ordering::SeqCst) {
            Err(ProviderError::Unauthorized(
                ProviderErrorContext::OpenRouter,
            ))
        } else {
            Ok(br#"{"data":{"usage":25,"limit":100,"is_management_key":false}}"#.to_vec())
        };
        Box::pin(async move { r })
    }
}

struct Rig {
    auth: Arc<OpenRouterAuthSession>,
    poller: OpenRouterUsagePoller,
    calls: Arc<AtomicUsize>,
    unauthorized: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
}

fn rig() -> Rig {
    let auth = Arc::new(OpenRouterAuthSession::new(Arc::new(
        InMemoryCredentialStore::default(),
    )));
    let calls = Arc::new(AtomicUsize::new(0));
    let unauthorized = Arc::new(AtomicBool::new(false));
    let enabled = Arc::new(AtomicBool::new(true));
    let (c, u) = (calls.clone(), unauthorized.clone());
    let factory = Arc::new(move |_: &_| {
        Ok(OpenRouterUsageClient::new(
            Arc::new(Fake {
                calls: c.clone(),
                unauthorized: u.clone(),
            }),
            Arc::new(NoCostEstimator),
        ))
    });
    let e = enabled.clone();
    let poller = OpenRouterUsagePoller::new(
        auth.clone(),
        Arc::new(ManualClock::from_unix(1_700_000_000)),
        factory,
        Arc::new(move || e.load(Ordering::SeqCst)),
    );
    Rig {
        auth,
        poller,
        calls,
        unauthorized,
        enabled,
    }
}

/// Without a key the refresh is skipped and the add-key hint is shown.
#[tokio::test]
async fn test_no_key_skips_with_hint() {
    let r = rig();
    assert_eq!(r.poller.refresh().await, RefreshOutcome::Skipped);
    assert!(r.auth.state().needs_sign_in);
    assert_eq!(
        r.poller.last_error().unwrap().to_string(),
        "Add an OpenRouter API key to load usage."
    );
    assert_eq!(r.calls.load(Ordering::SeqCst), 0);
}

/// A saved key publishes a snapshot, headline percent and refresh time.
#[tokio::test]
async fn test_refresh_publishes_snapshot() {
    let r = rig();
    r.auth.save_api_key("sk-or-v1-abc").unwrap();
    assert_eq!(r.poller.refresh().await, RefreshOutcome::Success);
    assert_eq!(r.poller.headline_used_percent(), Some(25.0));
    assert!(r.poller.last_refreshed_at().is_some() && r.poller.last_error().is_none());
    assert_eq!(r.poller.provider(), ProviderId::OpenRouter);
    assert!(r.poller.headline_resets_at().is_none() && r.poller.alert_account().is_none());
}

/// Disabled providers do not fetch.
#[tokio::test]
async fn test_disabled_provider_is_skipped() {
    let r = rig();
    r.auth.save_api_key("sk-or-v1-abc").unwrap();
    r.enabled.store(false, Ordering::SeqCst);
    assert_eq!(r.poller.refresh().await, RefreshOutcome::Skipped);
    assert_eq!(r.calls.load(Ordering::SeqCst), 0);
}

/// Three rejections invalidate the key; polling then stops until a new key.
#[tokio::test]
async fn test_three_strikes_stop_polling_until_new_key() {
    let r = rig();
    r.auth.save_api_key("sk-or-v1-abc").unwrap();
    r.unauthorized.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(r.poller.refresh().await, RefreshOutcome::Failure);
    }
    assert!(r.auth.state().needs_sign_in);
    assert_eq!(
        r.poller.last_error().unwrap().to_string(),
        "OpenRouter rejected the API key. Check or replace it."
    );
    let before = r.calls.load(Ordering::SeqCst);
    assert_eq!(r.poller.refresh().await, RefreshOutcome::Skipped);
    assert_eq!(r.calls.load(Ordering::SeqCst), before);
    r.unauthorized.store(false, Ordering::SeqCst);
    r.auth.save_api_key("sk-or-v1-new").unwrap();
    assert_eq!(r.poller.refresh().await, RefreshOutcome::Success);
}

/// A success resets the strike count.
#[tokio::test]
async fn test_success_resets_strikes() {
    let r = rig();
    r.auth.save_api_key("sk-or-v1-abc").unwrap();
    r.unauthorized.store(true, Ordering::SeqCst);
    r.poller.refresh().await;
    r.poller.refresh().await;
    r.unauthorized.store(false, Ordering::SeqCst);
    r.poller.refresh().await;
    r.unauthorized.store(true, Ordering::SeqCst);
    r.poller.refresh().await;
    r.poller.refresh().await;
    assert!(!r.auth.state().needs_sign_in);
}

/// Sign-out clears the snapshot, error and refresh time.
#[tokio::test]
async fn test_sign_out_clears_snapshot() {
    let r = rig();
    r.auth.save_api_key("sk-or-v1-abc").unwrap();
    r.poller.refresh().await;
    r.auth.sign_out();
    assert!(r.poller.snapshot().is_none());
    assert!(r.poller.last_refreshed_at().is_none() && r.poller.last_error().is_none());
}

/// Start, stop and menu flags are observable.
#[test]
fn test_start_stop_and_menu_flags() {
    let r = rig();
    r.poller.start();
    assert!(r.poller.is_running());
    r.poller.stop();
    assert!(!r.poller.is_running());
    r.poller.set_menu_open(true);
    assert!(r.poller.is_menu_open());
}

/// A fetcher that replaces the key (bumping the generation) mid-flight, then
/// answers 401, like a request issued with the old key.
struct StaleFake {
    auth: Arc<OpenRouterAuthSession>,
}

impl EndpointFetcher for StaleFake {
    fn fetch(&self, _: Endpoint) -> BoxFuture<'_, Result<Vec<u8>, ProviderError>> {
        self.auth.save_api_key("sk-or-v1-replacement").unwrap();
        Box::pin(async {
            Err(ProviderError::Unauthorized(
                ProviderErrorContext::OpenRouter,
            ))
        })
    }
}

/// A 401 for a key that was replaced mid-flight must not count against the
/// new key (PollerSessionGuardTests.testOpenRouterStaleUnauthorizedDoesNotInvalidateNewKey).
#[tokio::test]
async fn test_stale_unauthorized_does_not_invalidate_new_key() {
    let auth = Arc::new(OpenRouterAuthSession::new(Arc::new(
        InMemoryCredentialStore::default(),
    )));
    auth.save_api_key("sk-or-v1-old").unwrap();
    let a = auth.clone();
    let factory = Arc::new(move |_: &_| {
        Ok(OpenRouterUsageClient::new(
            Arc::new(StaleFake { auth: a.clone() }),
            Arc::new(NoCostEstimator),
        ))
    });
    let poller = OpenRouterUsagePoller::new(
        auth.clone(),
        Arc::new(ManualClock::from_unix(0)),
        factory,
        Arc::new(|| true),
    );
    for _ in 0..5 {
        poller.refresh().await;
    }
    assert!(auth.state().is_signed_in && !auth.state().needs_sign_in);
}

/// Three current 401s invalidate the key
/// (testOpenRouterCurrentUnauthorizedInvalidatesKey).
#[tokio::test]
async fn test_current_unauthorized_invalidates_key() {
    let r = rig();
    r.auth.save_api_key("sk-or-v1-abc").unwrap();
    r.unauthorized.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        r.poller.refresh().await;
    }
    assert!(r.auth.state().needs_sign_in);
}
