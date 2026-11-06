//! `GrokbotUsagePoller` on core polling: gating, the shared Cursor
//! three-strike counter, the 403 non-strike, session events and the loop.

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::polling::PollOutcome;
use tokenmon_core::storage::{InMemoryStringStore, TextStore};
use tokenmon_core::usage::WeekOffset;
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, AuthSessionState, CapturedSession,
    CredentialStoreError, ManualClock, ProviderAuthSession, ProviderError, ProviderErrorContext,
    ProviderId, ProviderUsagePoller, SessionGeneration,
};
use tokenmon_provider_grokbot::{
    FetchFn, GrokbotEntitlement, GrokbotPollerConfig, GrokbotPublisher, GrokbotSnapshot,
    GrokbotUsagePoller,
};

const CTX: ProviderErrorContext = ProviderErrorContext::Grokbot;

fn at(h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 25, h, 0, 0).unwrap()
}

/// A Cursor session fake over the real state machine that fires events.
struct FakeSession {
    state: Mutex<AuthSessionState>,
    listeners: Mutex<Vec<Arc<dyn AuthEventListener>>>,
    cookie: bool,
}

impl FakeSession {
    fn new(cookie: bool) -> Arc<Self> {
        let session = Arc::new(Self {
            state: Mutex::new(AuthSessionState::default()),
            listeners: Mutex::new(Vec::new()),
            cookie,
        });
        session
            .state
            .lock()
            .unwrap()
            .mark_signed_in(Some("a@b.com".into()));
        session
    }

    fn fire(&self, event: AuthEvent) {
        let listeners = self.listeners.lock().unwrap().clone();
        for listener in listeners {
            listener.on_auth_event(event);
        }
    }

    fn st(&self) -> AuthSessionState {
        self.state.lock().unwrap().clone()
    }
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        self.cookie.then(|| "WorkosCursorSessionToken=x".to_owned())
    }
    fn state(&self) -> AuthSessionState {
        self.st()
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        self.state.lock().unwrap().record_auth_failure(reason)
    }
    fn record_auth_success(&self) {
        self.state.lock().unwrap().record_auth_success();
    }
    fn is_current(&self, generation: SessionGeneration) -> bool {
        self.state.lock().unwrap().is_current(generation)
    }
    fn add_listener(&self, listener: Arc<dyn AuthEventListener>) {
        self.listeners.lock().unwrap().push(listener);
    }
}

impl ProviderAuthSession for FakeSession {
    fn set_needs_sign_in(&self, needs: bool) {
        self.state.lock().unwrap().needs_sign_in = needs;
    }
    fn save_account_email(&self, _email: &str) {}
    fn note_account_identity(&self, _identity: &str) {}
    fn apply_refreshed_cookies(&self, _set_cookie_headers: &[String]) {}
    fn mark_session_invalid(&self, _reason: Option<&str>) {}
    fn sign_out(&self) {}
    fn save(&self, _cookie_header: &str) -> Result<(), CredentialStoreError> {
        Ok(())
    }
    fn adopt(&self, _captured: &CapturedSession) -> Result<(), CredentialStoreError> {
        Ok(())
    }
    fn refresh_from_disk(&self) {}
    fn workspace_id(&self) -> Option<String> {
        None
    }
    fn set_workspace_id(&self, _id: &str) {}
}

fn snapshot(used: f64) -> GrokbotSnapshot {
    GrokbotSnapshot {
        fetched_at: at(15),
        used_percent: used,
        period_start: Some(Utc.with_ymd_and_hms(2026, 8, 20, 11, 0, 0).unwrap()),
        resets_at: Some(Utc.with_ymd_and_hms(2026, 8, 27, 11, 0, 0).unwrap()),
        entitlement: GrokbotEntitlement::Cursor,
        account_email: None,
        has_included_allowance: true,
    }
}

struct Harness {
    poller: Arc<GrokbotUsagePoller>,
    fetches: Arc<AtomicUsize>,
    enabled: Arc<AtomicBool>,
}

/// `interval` is what the settings return while the menu is open (closed
/// parks the loop).
fn harness(
    session: &Arc<FakeSession>,
    interval: Option<Duration>,
    f: impl Fn(usize) -> Result<GrokbotSnapshot, ProviderError> + Send + Sync + 'static,
) -> Harness {
    let clock = Arc::new(ManualClock::new(at(15)));
    let store: Arc<dyn TextStore> = Arc::new(InMemoryStringStore::default());
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fetches);
    let fetch: FetchFn = Arc::new(move |_cookie, _email| {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        let result = f(n);
        Box::pin(async move { result })
    });
    let enabled = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&enabled);
    let poller = GrokbotUsagePoller::new(GrokbotPollerConfig {
        session: session.clone(),
        publisher: GrokbotPublisher::new(&store, clock.clone()),
        clock,
        fetch,
        is_enabled: Arc::new(move || flag.load(Ordering::SeqCst)),
        poll_interval: Arc::new(move |open| if open { interval } else { None }),
    });
    Harness {
        poller,
        fetches,
        enabled,
    }
}

/// A success publishes the snapshot and headline, clears errors and the
/// sign-in prompt.
#[tokio::test]
async fn test_refresh_publishes_snapshot_and_headline() {
    let session = FakeSession::new(true);
    let h = harness(&session, None, |_| Ok(snapshot(12.0)));
    assert_eq!(h.poller.provider(), ProviderId::Grokbot);
    assert_eq!(h.poller.perform_refresh().await, PollOutcome::Success);
    assert_eq!(h.poller.headline_used_percent(), Some(12.0));
    assert_eq!(
        h.poller.headline_resets_at(),
        Some(Utc.with_ymd_and_hms(2026, 8, 27, 11, 0, 0).unwrap())
    );
    assert!(h.poller.last_refreshed_at().is_some() && h.poller.last_error().is_none());
    assert_eq!(h.poller.alert_account().as_deref(), Some("a@b.com"));
    assert!(!h.poller.daily_budget_days(WeekOffset::CURRENT).is_empty());
}

/// Disabled: no fetch. No cookie: sign-in prompt and an error while there is
/// no snapshot.
#[tokio::test]
async fn test_gating_disabled_and_missing_cookie() {
    let session = FakeSession::new(true);
    let h = harness(&session, None, |_| Ok(snapshot(1.0)));
    h.enabled.store(false, Ordering::SeqCst);
    assert_eq!(h.poller.perform_refresh().await, PollOutcome::Skipped);
    assert_eq!(h.fetches.load(Ordering::SeqCst), 0);

    let session = FakeSession::new(false);
    let h = harness(&session, None, |_| {
        unreachable!("no fetch without a cookie")
    });
    assert_eq!(h.poller.perform_refresh().await, PollOutcome::Skipped);
    assert!(session.st().needs_sign_in);
    assert!(matches!(
        h.poller.last_error(),
        Some(ProviderError::NotSignedIn(_))
    ));
}

/// Three Grokbot 401s invalidate the shared Cursor session; a plain 403
/// never counts.
#[tokio::test]
async fn test_unauthorized_strikes_shared_cursor_session_but_403_does_not() {
    let session = FakeSession::new(true);
    let h = harness(&session, None, |_| Err(ProviderError::Unauthorized(CTX)));
    for _ in 0..3 {
        h.poller.perform_refresh().await;
    }
    let state = session.st();
    assert!(state.needs_sign_in && !state.is_signed_in);

    let session = FakeSession::new(true);
    let h = harness(&session, None, |_| {
        Err(ProviderError::Network(
            CTX,
            "Grok Bot is not available".into(),
        ))
    });
    for _ in 0..5 {
        h.poller.perform_refresh().await;
    }
    let state = session.st();
    assert!(state.is_signed_in && state.consecutive_auth_failures == 0);
    assert!(h.poller.last_error().is_some());
}

/// Sign-out drops the snapshot but keeps history; an account reset clears the
/// history stores too.
#[tokio::test]
async fn test_session_events_clear_snapshot_and_account_reset_clears_history() {
    let session = FakeSession::new(true);
    let h = harness(&session, None, |n| Ok(snapshot(10.0 * n as f64)));
    h.poller.perform_refresh().await;
    h.poller.perform_refresh().await;
    session.fire(AuthEvent::SignedOut);
    assert!(h.poller.snapshot().is_none() && h.poller.last_refreshed_at().is_none());
    assert!(h.poller.with_publisher(|p| p.daily().spent_by_day().len()) > 0);
    h.poller.perform_refresh().await;
    session.fire(AuthEvent::AccountReset);
    assert!(h.poller.snapshot().is_none());
    assert_eq!(
        h.poller.with_publisher(|p| p.daily().spent_by_day().len()),
        0
    );
}

/// Opening the menu wakes a parked loop, which then refreshes at once.
#[tokio::test]
async fn test_set_menu_open_wakes_the_polling_loop() {
    let session = FakeSession::new(true);
    let h = harness(&session, Some(Duration::from_secs(60)), |_| {
        Ok(snapshot(5.0))
    });
    h.poller.start();
    assert_eq!(h.poller.polling_loop().delay_until_due(), None);
    assert_eq!(h.fetches.load(Ordering::SeqCst), 0);
    h.poller.set_menu_open(true);
    for _ in 0..200 {
        if h.fetches.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(h.fetches.load(Ordering::SeqCst), 1);
    h.poller.stop();
}
