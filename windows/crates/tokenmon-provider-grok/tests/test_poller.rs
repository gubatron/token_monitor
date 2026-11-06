//! Poller gating and session-guard tests (spec 02 sections 1.6 and 5.4,
//! plus the shared-poller session guard ported for Grok).
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use common::{clock_at, utc};
use tokenmon_core::{
    AUTH_FAILURE_THRESHOLD, AuthEvent, AuthEventListener, AuthSession, AuthSessionState, BoxFuture,
    ProviderError, ProviderErrorContext, SessionGeneration,
};
use tokenmon_provider_grok::history::GrokHistoryStore;
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;
use tokenmon_provider_grok::poller::{GrokFetch, GrokUsagePoller, HourlySink, RefreshOutcome};

struct FakeAuth {
    state: Mutex<AuthSessionState>,
    cookie: Mutex<Option<String>>,
    listeners: Mutex<Vec<Arc<dyn AuthEventListener>>>,
}

impl FakeAuth {
    fn signed_in(email: Option<&str>) -> Arc<Self> {
        let mut state = AuthSessionState::default();
        state.mark_signed_in(email.map(str::to_owned));
        Arc::new(Self {
            state: Mutex::new(state),
            cookie: Mutex::new(Some("sso=live".to_owned())),
            listeners: Mutex::new(Vec::new()),
        })
    }

    /// Delivers `event` to every registered listener, like the real session.
    fn fire(&self, event: AuthEvent) {
        let listeners = self.listeners.lock().unwrap().clone();
        for l in listeners {
            l.on_auth_event(event);
        }
    }

    fn sign_out_and_in_again(&self) {
        let mut state = self.state.lock().unwrap();
        state.mark_session_invalid(None);
        state.mark_signed_in(None);
    }
}

impl AuthSession for FakeAuth {
    fn cookie_header(&self) -> Option<String> {
        self.cookie.lock().unwrap().clone()
    }
    fn state(&self) -> AuthSessionState {
        self.state.lock().unwrap().clone()
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

type Script = dyn Fn(&FakeAuth) -> Result<WeeklyUsageSnapshot, ProviderError> + Send + Sync;

struct ScriptedFetch {
    auth: Arc<FakeAuth>,
    calls: AtomicUsize,
    script: Box<Script>,
}

impl GrokFetch for ScriptedFetch {
    fn fetch(
        &self,
        _cookie: String,
        _email: Option<String>,
    ) -> BoxFuture<'_, Result<WeeklyUsageSnapshot, ProviderError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            (self.script)(&self.auth)
        })
    }
}

#[derive(Default)]
struct FakeHourly {
    recorded: Mutex<Vec<f64>>,
    cleared: AtomicUsize,
}

impl HourlySink for FakeHourly {
    fn record(&self, used: f64, _at: DateTime<Utc>) {
        self.recorded.lock().unwrap().push(used);
    }
    fn clear(&self) {
        self.cleared.fetch_add(1, Ordering::SeqCst);
    }
}

fn snapshot(used: f64) -> WeeklyUsageSnapshot {
    WeeklyUsageSnapshot::new(utc(2026, 7, 15, 12, 0), used)
}

struct Rig {
    poller: Arc<GrokUsagePoller>,
    auth: Arc<FakeAuth>,
    fetch: Arc<ScriptedFetch>,
    hourly: Arc<FakeHourly>,
}

fn rig(
    script: impl Fn(&FakeAuth) -> Result<WeeklyUsageSnapshot, ProviderError> + Send + Sync + 'static,
) -> Rig {
    let auth = FakeAuth::signed_in(Some("me@x.co"));
    let fetch = Arc::new(ScriptedFetch {
        auth: auth.clone(),
        calls: AtomicUsize::new(0),
        script: Box::new(script),
    });
    let clock = clock_at(utc(2026, 7, 15, 12, 0));
    let hourly = Arc::new(FakeHourly::default());
    let poller = Arc::new(GrokUsagePoller::new(
        auth.clone(),
        GrokHistoryStore::in_memory(clock.clone()),
        clock,
        fetch.clone(),
        Some(hourly.clone()),
    ));
    Rig {
        poller,
        auth,
        fetch,
        hourly,
    }
}

fn unauthorized() -> ProviderError {
    ProviderError::Unauthorized(ProviderErrorContext::Grok)
}

/// A disabled Grok never fetches, even when signed in.
#[tokio::test]
async fn test_disabled_grok_never_fetches_even_when_signed_in() {
    let r = rig(|_| Ok(snapshot(10.0)));
    r.poller.set_enabled(false);
    assert_eq!(r.poller.perform_refresh().await, RefreshOutcome::Skipped);
    assert_eq!(r.fetch.calls.load(Ordering::SeqCst), 0);
    assert!(r.poller.snapshot().is_none());
}

/// Not signed in: the sign-in prompt error, no fetch.
#[tokio::test]
async fn test_signed_out_records_sign_in_message_without_fetching() {
    let r = rig(|_| Ok(snapshot(10.0)));
    r.auth.state.lock().unwrap().mark_session_invalid(None);
    assert_eq!(r.poller.perform_refresh().await, RefreshOutcome::Skipped);
    assert_eq!(
        r.poller.last_error().unwrap().to_string(),
        "Sign in to grok.com to load usage."
    );
    assert_eq!(r.fetch.calls.load(Ordering::SeqCst), 0);
}

/// A manual refresh publishes the snapshot, fills the account email, records
/// history and the hourly sink.
#[tokio::test]
async fn test_manual_refresh_publishes_and_records() {
    let r = rig(|_| Ok(snapshot(33.0)));
    assert_eq!(r.poller.perform_refresh().await, RefreshOutcome::Published);
    let s = r.poller.snapshot().unwrap();
    assert_eq!(s.account_email.as_deref(), Some("me@x.co"));
    assert_eq!(r.poller.headline_used_percent(), Some(33.0));
    assert_eq!(r.poller.alert_account().as_deref(), Some("me@x.co"));
    assert!(r.poller.last_refreshed_at().is_some());
    assert_eq!(*r.hourly.recorded.lock().unwrap(), [33.0]);
    r.poller.with_history(|h| assert_eq!(h.recent().len(), 1));
}

/// A failure after a good snapshot keeps the snapshot and reports the error.
#[tokio::test]
async fn test_failure_after_a_snapshot_still_reports_the_error() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let r = rig(move |_| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(snapshot(20.0))
        } else {
            Err(ProviderError::Network(
                ProviderErrorContext::Grok,
                "offline".into(),
            ))
        }
    });
    r.poller.perform_refresh().await;
    assert_eq!(r.poller.perform_refresh().await, RefreshOutcome::Failed);
    assert!(r.poller.snapshot().is_some());
    assert_eq!(
        r.poller.last_error().unwrap().to_string(),
        "Grok network error: offline"
    );
    assert!(!r.auth.state().needs_sign_in);
}

/// A dropped (cancelled) refresh records no error and leaves the poller
/// ready for the next one.
#[tokio::test(start_paused = true)]
async fn test_cancellation_is_not_reported_as_a_failure() {
    struct Hang;
    impl GrokFetch for Hang {
        fn fetch(
            &self,
            _: String,
            _: Option<String>,
        ) -> BoxFuture<'_, Result<WeeklyUsageSnapshot, ProviderError>> {
            Box::pin(std::future::pending())
        }
    }
    let auth = FakeAuth::signed_in(None);
    let clock = clock_at(utc(2026, 7, 15, 12, 0));
    let poller = GrokUsagePoller::new(
        auth.clone(),
        GrokHistoryStore::in_memory(clock.clone()),
        clock,
        Arc::new(Hang),
        None,
    );
    let timed_out =
        tokio::time::timeout(std::time::Duration::from_secs(1), poller.perform_refresh()).await;
    assert!(timed_out.is_err());
    assert!(poller.last_error().is_none());
    assert!(!auth.state().needs_sign_in);
}

/// A stale 401 (session replaced during the fetch) does not invalidate the new
/// session.
#[tokio::test]
async fn test_stale_unauthorized_does_not_invalidate_new_session() {
    let r = rig(|auth| {
        auth.sign_out_and_in_again();
        Err(unauthorized())
    });
    assert_eq!(r.poller.perform_refresh().await, RefreshOutcome::Skipped);
    let state = r.auth.state();
    assert!(state.is_signed_in && !state.needs_sign_in);
    assert_eq!(state.consecutive_auth_failures, 0);
}

/// Three current rejections in a row invalidate the session; hourly data is
/// kept through invalidation and sign-out.
#[tokio::test]
async fn test_current_unauthorized_invalidates_session_and_keeps_hourly() {
    let r = rig(|_| Err(unauthorized()));
    r.hourly.recorded.lock().unwrap().push(12.0);
    for attempt in 1..=AUTH_FAILURE_THRESHOLD {
        r.poller.perform_refresh().await;
        let invalidated = r.auth.state().needs_sign_in;
        assert_eq!(
            invalidated,
            attempt == AUTH_FAILURE_THRESHOLD,
            "attempt {attempt}"
        );
    }
    assert!(!r.auth.state().is_signed_in);
    r.auth.fire(AuthEvent::SignedOut);
    assert!(r.poller.snapshot().is_none());
    assert_eq!(r.hourly.cleared.load(Ordering::SeqCst), 0);
    assert_eq!(*r.hourly.recorded.lock().unwrap(), [12.0]);
}

/// A stale transport error does not clobber the new session's state.
#[tokio::test]
async fn test_stale_transport_error_does_not_clobber_new_session() {
    let r = rig(|auth| {
        auth.sign_out_and_in_again();
        Err(ProviderError::Network(
            ProviderErrorContext::Grok,
            "timed out".into(),
        ))
    });
    r.poller.perform_refresh().await;
    assert!(r.poller.last_error().is_none());
    assert!(r.auth.state().is_signed_in);
}

/// An account reset also clears the hourly data; a sign-out does not.
#[tokio::test]
async fn test_account_reset_clears_hourly_sign_out_keeps_it() {
    let r = rig(|_| Ok(snapshot(10.0)));
    r.poller.perform_refresh().await;
    r.auth.fire(AuthEvent::SignedOut);
    assert!(r.poller.snapshot().is_none() && r.poller.last_refreshed_at().is_none());
    assert_eq!(r.hourly.cleared.load(Ordering::SeqCst), 0);
    r.auth.fire(AuthEvent::AccountReset);
    assert_eq!(r.hourly.cleared.load(Ordering::SeqCst), 1);
    r.poller.with_history(|h| assert_eq!(h.recent().len(), 1));
}

/// `week(0)` is nil before a refresh; with a reset three days out it exists,
/// the previous week exists, and it is nil again after sign-out.
#[tokio::test]
async fn test_poller_builds_the_current_week() {
    let reset = utc(2026, 7, 18, 12, 0);
    let r = rig(move |_| Ok(snapshot(15.0).with_resets_at(Some(reset))));
    assert!(r.poller.week(0).is_none());
    r.poller.perform_refresh().await;
    let current = r.poller.week(0).unwrap();
    assert_eq!(current.days.len(), 7);
    assert!(r.poller.week(-1).is_some());
    assert_ne!(r.poller.week(-1).unwrap().week_start, current.week_start);
    r.auth.fire(AuthEvent::SignedOut);
    assert!(r.poller.week(0).is_none());
}

/// Regression (#65): the poller registers itself, so a session sign-out drops
/// the snapshot and an account reset also clears hourly data, with no direct
/// call into the poller.
#[tokio::test]
async fn test_poller_registers_for_session_events() {
    let r = rig(|_| Ok(snapshot(10.0)));
    assert_eq!(r.auth.listeners.lock().unwrap().len(), 1);
    r.poller.perform_refresh().await;
    r.auth.fire(AuthEvent::SignedOut);
    assert!(r.poller.snapshot().is_none());
    assert!(r.poller.headline_used_percent().is_none());
    assert_eq!(r.hourly.cleared.load(Ordering::SeqCst), 0);
    r.poller.perform_refresh().await;
    r.auth.fire(AuthEvent::AccountReset);
    assert!(r.poller.snapshot().is_none());
    assert_eq!(r.hourly.cleared.load(Ordering::SeqCst), 1);
}

/// The history flush API used on quit writes only when dirty and never panics
/// without a backing.
#[tokio::test]
async fn test_flush_history_is_safe_without_backing() {
    let r = rig(|_| Ok(snapshot(10.0)));
    r.poller.perform_refresh().await;
    r.poller.flush_history();
    r.poller.with_history(|h| assert_eq!(h.recent().len(), 1));
}
