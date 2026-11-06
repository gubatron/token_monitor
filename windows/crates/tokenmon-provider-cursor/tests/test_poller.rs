//! `CursorUsagePoller` on core polling (ports of `CursorUsagePollerTests.swift`
//! and the Cursor half of `PollerSessionGuardTests.swift`).

#![allow(clippy::unwrap_used)] // helpers and fakes outside `#[test]` fns

mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Duration as Span, Utc};
use common::at;
use tokenmon_core::polling::PollOutcome;
use tokenmon_core::storage::{DailyQuotaDeltaStore, InMemoryStringStore};
use tokenmon_core::usage::WeekOffset;
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, AuthSessionState, CapturedSession,
    CredentialStoreError, ManualClock, ProviderAuthSession, ProviderError, ProviderErrorContext,
    ProviderUsagePoller, SessionGeneration,
};
use tokenmon_provider_cursor::{
    CursorDayHourlyUsage, CursorFetchResult, CursorPollerConfig, CursorUsagePoller,
    DailyBudgetInputs, FetchFn, build_daily_budget_days, parse_summary,
};

const CTX: ProviderErrorContext = ProviderErrorContext::Cursor;

fn day(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    at(y, m, d, 0, 0)
}

fn spent(entries: &[(DateTime<Utc>, f64)]) -> HashMap<DateTime<Utc>, f64> {
    entries.iter().copied().collect()
}

fn bars(
    observed: &[(DateTime<Utc>, f64)],
    interrupted: Option<DateTime<Utc>>,
    used: f64,
    cycle: (DateTime<Utc>, DateTime<Utc>),
    week_offset: i32,
    now: DateTime<Utc>,
) -> Option<Vec<tokenmon_core::usage::DailyBudgetDay>> {
    build_daily_budget_days(
        &ManualClock::new(now),
        DailyBudgetInputs {
            observed_by_day: &spent(observed),
            interrupted_window_start: interrupted,
            estimated_weight_by_day: &BTreeMap::new(),
            used_percent: used,
            cycle_start: Some(cycle.0),
            cycle_end: Some(cycle.1),
            week_offset: WeekOffset::new(week_offset),
            now,
        },
    )
}

/// A completed tracked day reads its real pool-percent growth (68 to 71 gives
/// 3), not a list-price estimate.
#[test]
fn test_completed_day_uses_pool_percent_delta() {
    let now = at(2026, 8, 28, 18, 0);
    let clock = Arc::new(ManualClock::new(now));
    let mut store =
        DailyQuotaDeltaStore::new(Arc::new(InMemoryStringStore::default()), "cursor", clock);
    store.record(66.0, at(2026, 8, 27, 9, 0), None, 7);
    store.record(68.0, at(2026, 8, 27, 18, 0), None, 7);
    store.record(68.0, at(2026, 8, 28, 9, 0), None, 7);
    store.record(71.0, now, None, 7);
    let observed: HashMap<_, _> = store.spent_by_day().into_iter().collect();
    let weights = BTreeMap::from([(now.date_naive(), 100.0)]);
    let days = build_daily_budget_days(
        &ManualClock::new(now),
        DailyBudgetInputs {
            observed_by_day: &observed,
            interrupted_window_start: None,
            estimated_weight_by_day: &weights,
            used_percent: 71.0,
            cycle_start: Some(now - Span::days(16)),
            cycle_end: Some(now + Span::days(14)),
            week_offset: WeekOffset::CURRENT,
            now,
        },
    )
    .unwrap();
    let today = days.iter().find(|d| d.date == day(2026, 8, 28)).unwrap();
    assert!((today.spent_usd - 3.0).abs() < 0.001, "{}", today.spent_usd);
}

/// An early reset (cycle restarted Oct 2, old cycle began Sep 10): old-cycle
/// days of the displayed week are dimmed history and leave the new pool alone.
#[test]
fn test_early_reset_keeps_old_cycle_days_as_history_and_restarts_pool_math() {
    let observed = [
        (day(2026, 9, 29), 2.0),
        (day(2026, 10, 1), 5.0),
        (day(2026, 10, 2), 1.0),
        (day(2026, 10, 3), 3.0),
    ];
    let days = bars(
        &observed,
        Some(at(2026, 9, 10, 17, 0)),
        4.0,
        (at(2026, 10, 2, 17, 0), at(2026, 11, 2, 17, 0)),
        0,
        at(2026, 10, 4, 12, 0),
    )
    .unwrap();
    // Week of Monday Sep 28.
    assert_eq!(days.len(), 7);
    let (sep29, oct1, oct3) = (&days[1], &days[3], &days[5]);
    assert!(sep29.is_prior_window);
    assert!((sep29.spent_usd - 2.0).abs() < 0.001);
    assert!(oct1.is_prior_window);
    assert!((oct1.spent_usd - 5.0).abs() < 0.001);
    assert!(!oct3.is_prior_window);
    assert!((oct3.spent_usd - 3.0).abs() < 0.001);
    assert!((days[0].budget_usd - 100.0 / 31.0).abs() < 1e-9);
    let live: f64 = days
        .iter()
        .filter(|d| !d.is_prior_window)
        .map(|d| d.spent_usd)
        .sum();
    assert!(live <= 4.001, "{live}");
}

/// With no early reset, days of the displayed week before the cycle start read
/// 0 and nothing is flagged.
#[test]
fn test_without_early_reset_old_days_stay_hidden() {
    let days = bars(
        &[(day(2026, 10, 1), 5.0), (day(2026, 10, 3), 4.0)],
        None,
        4.0,
        (at(2026, 10, 2, 17, 0), at(2026, 11, 2, 17, 0)),
        0,
        at(2026, 10, 4, 12, 0),
    )
    .unwrap();
    assert!(days.iter().all(|d| !d.is_prior_window));
    assert_eq!(days[3].spent_usd, 0.0);
}

/// Chevron-left shows the previous Monday week; a day before the cycle start
/// uses preserved history and is not dimmed.
#[test]
fn test_build_daily_budget_days_week_offset_shows_previous_monday() {
    let days = bars(
        &[(day(2026, 8, 18), 4.0), (day(2026, 8, 25), 3.0)],
        None,
        3.0,
        (at(2026, 8, 22, 17, 0), at(2026, 9, 22, 17, 0)),
        -1,
        at(2026, 8, 28, 12, 0),
    )
    .unwrap();
    assert_eq!(days[0].date, day(2026, 8, 17));
    let aug18 = days.iter().find(|d| d.date == day(2026, 8, 18)).unwrap();
    assert!((aug18.spent_usd - 4.0).abs() < 0.001);
    assert!(!aug18.is_prior_window);
}

/// A session fake over the real state machine that fires [`AuthEvent`]s.
struct FakeSession {
    state: Mutex<AuthSessionState>,
    listeners: Mutex<Vec<Arc<dyn AuthEventListener>>>,
    cookie: bool,
}

impl FakeSession {
    fn signed_in() -> Arc<Self> {
        let session = Arc::new(Self {
            state: Mutex::new(AuthSessionState::default()),
            listeners: Mutex::new(Vec::new()),
            cookie: true,
        });
        session.sign_in();
        session
    }

    fn without_cookie() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AuthSessionState::default()),
            listeners: Mutex::new(Vec::new()),
            cookie: false,
        })
    }

    fn sign_in(&self) {
        let mut state = self.state.lock().unwrap();
        let email = state.account_email.clone();
        state.mark_signed_in(email);
    }

    fn sign_out(&self) {
        self.state.lock().unwrap().mark_session_invalid(None);
    }

    fn fire(&self, event: AuthEvent) {
        let listeners = self.listeners.lock().unwrap().clone();
        for listener in listeners {
            listener.on_auth_event(event);
        }
    }

    fn snapshot_state(&self) -> AuthSessionState {
        self.state.lock().unwrap().clone()
    }
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        (self.cookie && self.state.lock().unwrap().is_signed_in)
            .then(|| "WorkosCursorSessionToken=x".to_owned())
    }
    fn state(&self) -> AuthSessionState {
        self.snapshot_state()
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
    fn save_account_email(&self, email: &str) {
        let changed = {
            let mut state = self.state.lock().unwrap();
            let changed = state
                .account_email
                .as_deref()
                .is_some_and(|old| old != email);
            state.account_email = Some(email.to_owned());
            changed
        };
        if changed {
            self.fire(AuthEvent::AccountReset);
        }
    }
    fn note_account_identity(&self, _identity: &str) {}
    fn apply_refreshed_cookies(&self, _set_cookie_headers: &[String]) {}
    fn mark_session_invalid(&self, _reason: Option<&str>) {
        self.state.lock().unwrap().mark_session_invalid(None);
    }
    fn sign_out(&self) {
        FakeSession::sign_out(self);
    }
    fn save(&self, _cookie_header: &str) -> Result<(), CredentialStoreError> {
        self.state.lock().unwrap().mark_signed_in(None);
        Ok(())
    }
    fn adopt(&self, _captured: &CapturedSession) -> Result<(), CredentialStoreError> {
        Ok(())
    }
    fn refresh_from_disk(&self) {}
}

fn now() -> DateTime<Utc> {
    at(2026, 8, 28, 12, 0)
}

fn fetch_result(email: Option<&str>) -> CursorFetchResult {
    let body = serde_json::json!({
        "billingCycleStart": at(2026, 8, 22, 17, 0).to_rfc3339(),
        "billingCycleEnd": at(2026, 9, 22, 17, 0).to_rfc3339(),
        "individualUsage": {"plan": {"totalPercentUsed": 42}},
    });
    let mut snapshot = parse_summary(body.to_string().as_bytes(), now()).unwrap();
    snapshot.account_email = email.map(str::to_owned);
    CursorFetchResult {
        snapshot,
        hourly: CursorDayHourlyUsage::empty(day(2026, 8, 28)),
        estimated_weight_by_day: BTreeMap::new(),
    }
}

struct Harness {
    poller: Arc<CursorUsagePoller>,
    session: Arc<FakeSession>,
    fetches: Arc<AtomicUsize>,
}

fn harness_with(
    session: Arc<FakeSession>,
    enabled: bool,
    interval: Option<Duration>,
    fetch: impl Fn(String) -> Result<CursorFetchResult, ProviderError> + Send + Sync + 'static,
) -> Harness {
    let clock = Arc::new(ManualClock::new(now()));
    let daily = DailyQuotaDeltaStore::new(
        Arc::new(InMemoryStringStore::default()),
        "cursor",
        clock.clone(),
    );
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fetches);
    let fetch: FetchFn = Arc::new(move |cookie| {
        counter.fetch_add(1, Ordering::SeqCst);
        let result = fetch(cookie);
        Box::pin(async move { result })
    });
    let poller = CursorUsagePoller::new(CursorPollerConfig {
        session: session.clone(),
        daily,
        clock,
        fetch,
        clear_event_cache: Arc::new(|| {}),
        is_enabled: Arc::new(move || enabled),
        poll_interval: Arc::new(move |open| interval.filter(|_| open)),
    });
    Harness {
        poller,
        session,
        fetches,
    }
}

/// A success for a session replaced during the request belongs to the old
/// account: nothing is published and the new session stays signed in.
#[tokio::test]
async fn test_cursor_stale_success_is_dropped() {
    let session = FakeSession::signed_in();
    let swapper = Arc::clone(&session);
    let h = harness_with(session, true, None, move |_| {
        swapper.sign_out();
        swapper.sign_in();
        Ok(fetch_result(None))
    });
    h.poller.refresh_now().await;
    assert!(h.poller.snapshot().is_none());
    assert!(h.session.snapshot_state().is_signed_in);
}

/// A 401 for a session replaced during the request must not strike the new one.
#[tokio::test]
async fn test_cursor_stale_unauthorized_does_not_invalidate_new_session() {
    let session = FakeSession::signed_in();
    let swapper = Arc::clone(&session);
    let h = harness_with(session, true, None, move |_| {
        swapper.sign_out();
        swapper.sign_in();
        Err(ProviderError::Unauthorized(CTX))
    });
    h.poller.refresh_now().await;
    let state = h.session.snapshot_state();
    assert!(state.is_signed_in);
    assert!(!state.needs_sign_in);
    assert_eq!(state.consecutive_auth_failures, 0);
    assert!(h.poller.last_error().is_none());
}

/// A rejection of the live session counts toward invalidating it and is
/// reported through `last_error`.
#[tokio::test]
async fn test_cursor_current_unauthorized_invalidates_session_after_three() {
    let h = harness_with(FakeSession::signed_in(), true, None, |_| {
        Err(ProviderError::Unauthorized(CTX))
    });
    for _ in 0..3 {
        h.poller.refresh_now().await;
    }
    assert!(h.session.snapshot_state().needs_sign_in);
    assert_eq!(
        h.poller.last_error(),
        Some(ProviderError::Unauthorized(CTX))
    );
}

/// A successful refresh publishes the snapshot, the bars and the label, and
/// clears the sign-in flag.
#[tokio::test]
async fn test_refresh_publishes_snapshot_bars_and_label() {
    let h = harness_with(FakeSession::signed_in(), true, None, |cookie| {
        assert_eq!(cookie, "WorkosCursorSessionToken=x");
        Ok(fetch_result(Some("a@b.com")))
    });
    h.poller.refresh_now().await;
    assert_eq!(h.poller.headline_used_percent(), Some(42.0));
    assert_eq!(h.poller.headline_resets_at(), Some(at(2026, 9, 22, 17, 0)));
    assert_eq!(h.poller.alert_account().as_deref(), Some("a@b.com"));
    assert_eq!(h.poller.data_source_label(), Some("Cursor dashboard"));
    assert_eq!(h.poller.daily_budget_days().unwrap().len(), 7);
    assert_eq!(
        h.poller
            .daily_budget_days_at(WeekOffset::new(-1))
            .unwrap()
            .len(),
        7
    );
    assert!(h.poller.last_refreshed_at().is_some());
    assert!(!h.session.snapshot_state().needs_sign_in);
}

/// Without a cookie the poller flags sign-in and does not fetch.
#[tokio::test]
async fn test_refresh_without_cookie_flags_sign_in_and_skips() {
    let h = harness_with(FakeSession::without_cookie(), true, None, |_| {
        Ok(fetch_result(None))
    });
    h.poller.refresh_now().await;
    assert_eq!(h.fetches.load(Ordering::SeqCst), 0);
    assert!(h.session.snapshot_state().needs_sign_in);
    assert_eq!(h.poller.last_error(), Some(ProviderError::NotSignedIn(CTX)));
}

/// A disabled provider never fetches.
#[tokio::test]
async fn test_disabled_provider_skips_refresh() {
    let h = harness_with(FakeSession::signed_in(), false, None, |_| {
        Ok(fetch_result(None))
    });
    assert_eq!(h.poller.perform_refresh().await, PollOutcome::Skipped);
    assert_eq!(h.fetches.load(Ordering::SeqCst), 0);
}

/// A different account fires `AccountReset` (no deadlock under the poller); a
/// sign-out clears the snapshot; the next refresh publishes again.
#[tokio::test]
async fn test_account_change_and_sign_out_reset_the_snapshot() {
    let emails = Arc::new(Mutex::new(vec!["new@b.com", "a@b.com"]));
    let queue = Arc::clone(&emails);
    let h = harness_with(FakeSession::signed_in(), true, None, move |_| {
        let email = queue.lock().unwrap().pop().unwrap();
        let mut result = fetch_result(Some(email));
        result.snapshot.used_percent = 10.0;
        Ok(result)
    });
    h.poller.refresh_now().await;
    assert_eq!(h.poller.alert_account().as_deref(), Some("a@b.com"));
    h.session.fire(AuthEvent::SignedOut);
    assert!(h.poller.snapshot().is_none());
    h.poller.refresh_now().await;
    assert_eq!(h.poller.alert_account().as_deref(), Some("new@b.com"));
    assert!(h.poller.snapshot().is_some());
}

/// Opening the menu wakes a parked loop, which then refreshes at once.
#[tokio::test]
async fn test_set_menu_open_wakes_the_polling_loop() {
    let h = harness_with(
        FakeSession::signed_in(),
        true,
        Some(Duration::from_secs(60)),
        |_| Ok(fetch_result(None)),
    );
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
