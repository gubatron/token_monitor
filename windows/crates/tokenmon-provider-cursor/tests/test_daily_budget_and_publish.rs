//! Daily-budget blending, publish carry-over and the `UsageFetcher` binding
//! (`CursorUsagePollerTests.swift`, `PollerSessionGuardTests.swift`).

#![allow(clippy::unwrap_used)] // helpers and fakes outside `#[test]` fns

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use common::{at, harness};
use tokenmon_core::{
    AUTH_FAILURE_THRESHOLD, AuthEventListener, AuthSession, AuthSessionState, ManualClock,
    ProviderError, ProviderErrorContext, SessionGeneration, UsageFetcher,
};
use tokenmon_provider_cursor::{
    BlendInputs, CursorCostStats, CursorDayHourlyUsage, CursorFetchResult, CursorFetcher,
    PreviousPublish, account_email_changed, blend_daily_budget, carry_over, cycle_length_days,
    parse_summary,
};

fn day(m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, m, d).unwrap()
}

fn blend(
    observed: &[(NaiveDate, f64)],
    estimated: &[(NaiveDate, f64)],
    used: f64,
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
    interrupted: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<tokenmon_provider_cursor::BlendedBudget> {
    let observed: BTreeMap<_, _> = observed.iter().copied().collect();
    let estimated: BTreeMap<_, _> = estimated.iter().copied().collect();
    blend_daily_budget(
        &ManualClock::from_unix(0),
        BlendInputs {
            observed_by_day: &observed,
            interrupted_window_start: interrupted,
            estimated_weight_by_day: &estimated,
            used_percent: used,
            cycle_start: start,
            cycle_end: end,
            now: at(2026, 8, 28, 12, 0),
        },
    )
}

fn aug_cycle() -> (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
) {
    (Some(at(2026, 8, 22, 17, 0)), Some(at(2026, 9, 22, 17, 0)))
}

/// The measured day keeps its delta; untracked days share the remainder by weight.
#[test]
fn test_tracked_day_uses_measured_delta_and_others_backfilled() {
    let (s, e) = aug_cycle();
    let est: Vec<_> = [22, 23, 24, 26, 27]
        .iter()
        .map(|d| (day(8, *d), 100.0))
        .collect();
    let out = blend(
        &[(day(8, 23), 9.0), (day(8, 25), 3.0)],
        &est,
        9.0,
        s,
        e,
        None,
    )
    .unwrap();
    assert_eq!(out.blended[&day(8, 25)], 3.0);
    assert!((out.blended[&day(8, 26)] - 1.2).abs() < 1e-9);
    assert_eq!(out.end, at(2026, 9, 22, 17, 0));
}

/// The first (partially tracked) observed day is replaced by its estimate.
#[test]
fn test_first_tracked_day_uses_estimate() {
    let (s, e) = aug_cycle();
    let est = [(day(8, 24), 50.0), (day(8, 25), 50.0)];
    let out = blend(&[(day(8, 25), 1.0)], &est, 4.0, s, e, None).unwrap();
    assert!((out.blended[&day(8, 25)] - 2.0).abs() < 1e-9);
}

/// Without any cycle date no month is invented.
#[test]
fn test_build_daily_budget_days_nil_without_cycle_dates() {
    assert!(blend(&[(day(8, 25), 1.0)], &[], 4.0, None, None, None).is_none());
}

/// Tracked usage above the pool percent is scaled down, never back-filled.
#[test]
fn test_tracked_exceeding_pool_percent_is_rescaled() {
    let (s, e) = aug_cycle();
    let obs = [(day(8, 23), 1.0), (day(8, 25), 6.0), (day(8, 26), 4.0)];
    let out = blend(&obs, &[], 5.0, s, e, None).unwrap();
    assert!((out.blended[&day(8, 25)] - 3.0).abs() < 1e-9);
    assert!((out.blended[&day(8, 26)] - 2.0).abs() < 1e-9);
    assert!((out.blended.values().sum::<f64>() - 5.0).abs() < 1e-9);
}

/// Days of an interrupted earlier window are reported as `prior`.
#[test]
fn test_early_reset_keeps_old_cycle_days_as_history() {
    let obs = [
        (day(9, 29), 2.0),
        (day(10, 1), 5.0),
        (day(10, 3), 3.0),
        (day(10, 4), 1.0),
    ];
    let out = blend(
        &obs,
        &[],
        4.0,
        Some(at(2026, 10, 2, 17, 0)),
        Some(at(2026, 11, 2, 17, 0)),
        Some(at(2026, 9, 10, 8, 0)),
    )
    .unwrap();
    assert_eq!(out.prior[&day(9, 29)], 2.0);
    assert_eq!(out.prior[&day(10, 1)], 5.0);
    assert!(!out.blended.contains_key(&day(9, 29)));
    assert!(out.blended.values().sum::<f64>() <= 4.0 + 1e-9);
}

/// Old days stay hidden without an interrupted-window marker.
#[test]
fn test_without_early_reset_old_days_stay_hidden() {
    let obs = [(day(9, 29), 2.0), (day(10, 1), 5.0), (day(10, 3), 3.0)];
    let out = blend(
        &obs,
        &[],
        4.0,
        Some(at(2026, 10, 2, 17, 0)),
        Some(at(2026, 11, 2, 17, 0)),
        None,
    )
    .unwrap();
    assert!(out.prior.is_empty());
    assert!(!out.blended.contains_key(&day(10, 1)));
}

/// Extreme or non-finite inputs never produce negative or non-finite bars.
#[test]
fn test_blend_output_is_finite_and_non_negative_for_extreme_inputs() {
    let (s, e) = aug_cycle();
    let obs = [
        (day(8, 23), f64::MAX),
        (day(8, 24), f64::NAN),
        (day(8, 25), -5.0),
        (day(8, 26), f64::INFINITY),
    ];
    let est = [(day(8, 27), f64::MAX), (day(8, 28), f64::MAX)];
    let out = blend(&obs, &est, 80.0, s, e, None).unwrap();
    assert!(out.blended.values().all(|v| v.is_finite() && *v >= 0.0));
}

fn prev(cents: f64, hour_day: chrono::DateTime<chrono::Utc>) -> PreviousPublish {
    let mut snap = parse_summary(b"{}", at(2026, 8, 10, 12, 0)).unwrap();
    snap.cost_stats = Some(CursorCostStats {
        metered_cycle_usd: cents / 100.0,
        cycle_tokens: 70,
        ..Default::default()
    });
    let mut hourly = CursorDayHourlyUsage::empty(hour_day);
    hourly.hour_weights[5] = 3.0;
    PreviousPublish {
        snapshot: snap,
        hourly,
        estimated_weight_by_day: BTreeMap::from([(day(8, 9), 1.0)]),
    }
}

fn unavailable(hour_day: chrono::DateTime<chrono::Utc>) -> CursorFetchResult {
    CursorFetchResult {
        snapshot: parse_summary(b"{}", at(2026, 8, 10, 13, 0)).unwrap(),
        hourly: CursorDayHourlyUsage::empty(hour_day),
        estimated_weight_by_day: BTreeMap::new(),
    }
}

/// Unavailable events keep the previous stats and same-day hourly chart.
#[test]
fn test_poller_keeps_previous_event_figures_when_events_are_unavailable() {
    let d = at(2026, 8, 10, 0, 0);
    let out = carry_over(unavailable(d), Some(&prev(700.0, d)));
    let stats = out.snapshot.cost_stats.unwrap();
    assert_eq!((stats.metered_cycle_usd, stats.cycle_tokens), (7.0, 70));
    assert_eq!(out.hourly.hour_weights[5], 3.0);
    assert_eq!(out.estimated_weight_by_day.len(), 1);
}

/// A new day drops the stale hourly chart; a new cycle drops everything.
#[test]
fn test_carry_over_rules_for_new_day_and_new_cycle() {
    let yesterday = at(2026, 8, 9, 0, 0);
    let today = at(2026, 8, 10, 0, 0);
    let out = carry_over(unavailable(today), Some(&prev(700.0, yesterday)));
    assert!(out.snapshot.cost_stats.is_some());
    assert!(out.hourly.is_empty());
    let mut other_cycle = unavailable(today);
    other_cycle.snapshot.billing_cycle_start = Some(at(2026, 7, 1, 0, 0));
    let out = carry_over(other_cycle, Some(&prev(700.0, today)));
    assert!(out.snapshot.cost_stats.is_none());
}

/// Fresh event data is never overwritten by carry-over.
#[test]
fn test_carry_over_leaves_fresh_event_data_alone() {
    let d = at(2026, 8, 10, 0, 0);
    let mut fresh = unavailable(d);
    fresh.snapshot.cost_stats = Some(CursorCostStats {
        metered_cycle_usd: 1.0,
        ..Default::default()
    });
    let out = carry_over(fresh, Some(&prev(700.0, d)));
    assert_eq!(out.snapshot.cost_stats.unwrap().metered_cycle_usd, 1.0);
}

/// Cycle length defaults to 30 days; account changes are detected.
#[test]
fn test_cycle_length_and_account_email_rules() {
    let clock = ManualClock::from_unix(0);
    assert_eq!(cycle_length_days(&clock, None, None), 30);
    assert_eq!(
        cycle_length_days(
            &clock,
            Some(at(2026, 8, 22, 17, 0)),
            Some(at(2026, 9, 22, 17, 0))
        ),
        31
    );
    assert_eq!(
        cycle_length_days(
            &clock,
            Some(at(2026, 9, 1, 0, 0)),
            Some(at(2026, 8, 1, 0, 0))
        ),
        30
    );
    assert!(account_email_changed(Some("a@b.c"), None));
    assert!(account_email_changed(Some("a@b.c"), Some("x@y.z")));
    assert!(!account_email_changed(Some("a@b.c"), Some("a@b.c")));
    assert!(!account_email_changed(None, Some("a@b.c")));
}

/// Minimal session fake with the real three-strike state machine.
struct FakeSession {
    cookie: Option<String>,
    state: Mutex<AuthSessionState>,
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        self.cookie.clone()
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
    fn add_listener(&self, _: Arc<dyn AuthEventListener>) {}
}

fn session(cookie: Option<&str>) -> Arc<FakeSession> {
    let mut state = AuthSessionState::default();
    state.mark_signed_in(None);
    Arc::new(FakeSession {
        cookie: cookie.map(str::to_owned),
        state: Mutex::new(state),
    })
}

/// No cookie means NotSignedIn and no request.
#[tokio::test]
async fn test_fetcher_without_cookie_is_not_signed_in() {
    let h = harness(at(2026, 8, 10, 12, 0), vec![]).await;
    let clock = h.clock.clone();
    let fetcher = CursorFetcher::new(h.client, session(None), clock);
    let e = fetcher.fetch().await.unwrap_err();
    assert_eq!(e, ProviderError::NotSignedIn(ProviderErrorContext::Cursor));
    assert!(fetcher.snapshot().is_none());
}

/// A success is remembered and resets the strike counter.
#[tokio::test]
async fn test_fetcher_success_records_auth_success_and_keeps_snapshot() {
    let h = harness(at(2026, 8, 10, 12, 0), vec![]).await;
    let s = session(Some("WorkosCursorSessionToken=t"));
    s.record_auth_failure("x");
    let fetcher = CursorFetcher::new(h.client, s.clone(), h.clock.clone());
    let snap = fetcher.fetch().await.unwrap();
    assert_eq!(snap.used_percent, 5.0);
    assert_eq!(fetcher.snapshot().unwrap(), snap);
    assert_eq!(s.state().consecutive_auth_failures, 0);
}

/// Three consecutive rejections invalidate the session.
#[tokio::test]
async fn test_fetcher_current_unauthorized_invalidates_session_after_three() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let clock = Arc::new(ManualClock::new(at(2026, 8, 10, 12, 0)));
    let client = tokenmon_provider_cursor::CursorUsageClient::new(
        tokenmon_provider_cursor::CursorHttp::new(&server.uri(), "t"),
        clock.clone(),
    );
    let s = session(Some("c=1"));
    let fetcher = CursorFetcher::new(client, s.clone(), clock);
    for _ in 0..AUTH_FAILURE_THRESHOLD {
        assert!(fetcher.fetch().await.is_err());
    }
    assert!(s.state().needs_sign_in);
    assert!(fetcher.snapshot().is_none());
}
