//! `ClaudeUsagePoller` against a scripted fetch, a fake cookie session and
//! in-memory stores (ports of `ClaudeUsagePollerTests`,
//! `PollerSessionGuardTests` and `AuthInvalidationTests` for Claude).
#![allow(clippy::unwrap_used, clippy::unused_async)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use tokenmon_core::polling::testing::ManualScheduler;
use tokenmon_core::settings::{AppSettings, SettingsFile};
use tokenmon_core::storage::{
    DailyQuotaDeltaStore, HourlyDeltaActivityStore, InMemoryStringStore, QuotaWindow,
};
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay, WeekOffset};
use tokenmon_core::{
    AUTH_FAILURE_THRESHOLD, AuthEvent, AuthEventListener, AuthSession, AuthSessionState,
    CapturedSession, Clock, CredentialStoreError, ManualClock, ProviderAuthSession, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller, SessionGeneration,
};
use tokenmon_provider_claude::budget::build_daily_budget_days;
use tokenmon_provider_claude::{
    ClaudeCodeUsageScan, ClaudeCodeUsageSummary, ClaudePollerDeps, ClaudeUsagePoller,
    ClaudeUsageResponse, ClaudeUsageWindow, FetchUsageFn, LoadCodeUsageFn, UsageTotals,
    no_code_usage, parse_usage_body,
};

const CTX: ProviderErrorContext = ProviderErrorContext::Claude;

// ---------------------------------------------------------------- fixtures

#[derive(Default)]
struct SessionInner {
    state: AuthSessionState,
    cookie: Option<String>,
    org: Option<String>,
    listeners: Vec<Arc<dyn AuthEventListener>>,
}

/// In-memory Claude session: the real pure state machine plus the sign-out and
/// account-reset events (`ClaudeAuthSession.signOut` / `adopt`).
#[derive(Default)]
struct FakeSession {
    inner: Mutex<SessionInner>,
}

impl FakeSession {
    fn signed_in(cookie: &str) -> Arc<Self> {
        let s = Arc::new(Self::default());
        s.save(cookie);
        s
    }

    fn save(&self, cookie: &str) {
        let mut g = self.inner.lock().unwrap();
        g.cookie = Some(cookie.to_owned());
        g.state.mark_signed_in(None);
    }

    fn sign_out(&self) {
        let listeners = {
            let mut g = self.inner.lock().unwrap();
            g.cookie = None;
            g.state.mark_session_invalid(None);
            g.listeners.clone()
        };
        for l in listeners {
            l.on_auth_event(AuthEvent::SignedOut);
        }
    }

    /// Signs in with `org`; a different organization than the remembered one
    /// fires `AccountReset` first.
    fn adopt(&self, org: &str) {
        let (reset, listeners) = {
            let mut g = self.inner.lock().unwrap();
            let reset = g.org.as_deref().is_some_and(|o| o != org);
            g.org = Some(org.to_owned());
            g.cookie = Some(format!("sessionKey=live; lastActiveOrg={org}"));
            g.state.mark_signed_in(None);
            (reset, g.listeners.clone())
        };
        if reset {
            for l in listeners {
                l.on_auth_event(AuthEvent::AccountReset);
            }
        }
    }

    fn snapshot(&self) -> AuthSessionState {
        self.inner.lock().unwrap().state.clone()
    }
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        self.inner.lock().unwrap().cookie.clone()
    }
    fn state(&self) -> AuthSessionState {
        self.snapshot()
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        let invalid = g.state.record_auth_failure(reason);
        if invalid {
            g.cookie = None;
        }
        invalid
    }
    fn record_auth_success(&self) {
        self.inner.lock().unwrap().state.record_auth_success();
    }
    fn is_current(&self, generation: SessionGeneration) -> bool {
        self.inner.lock().unwrap().state.is_current(generation)
    }
    fn add_listener(&self, listener: Arc<dyn AuthEventListener>) {
        self.inner.lock().unwrap().listeners.push(listener);
    }
}

impl ProviderAuthSession for FakeSession {
    fn set_needs_sign_in(&self, needs: bool) {
        self.inner.lock().unwrap().state.needs_sign_in = needs;
    }
    fn save_account_email(&self, _email: &str) {}
    fn note_account_identity(&self, _identity: &str) {}
    fn apply_refreshed_cookies(&self, _set_cookie_headers: &[String]) {}
    fn mark_session_invalid(&self, reason: Option<&str>) {
        let mut g = self.inner.lock().unwrap();
        g.cookie = None;
        g.state.mark_session_invalid(reason);
    }
    fn sign_out(&self) {
        FakeSession::sign_out(self);
    }
    fn save(&self, cookie_header: &str) -> Result<(), CredentialStoreError> {
        let mut g = self.inner.lock().unwrap();
        g.cookie = Some(cookie_header.to_owned());
        g.state.mark_signed_in(None);
        Ok(())
    }
    fn adopt(&self, _captured: &CapturedSession) -> Result<(), CredentialStoreError> {
        Ok(())
    }
    fn refresh_from_disk(&self) {}
}

type Outcome = Result<(ClaudeUsageResponse, DateTime<Utc>), ProviderError>;

/// Feeds a fixed sequence of outcomes to the fetch seam; the last repeats.
#[derive(Clone)]
struct Script {
    outcomes: Arc<Mutex<VecDeque<Outcome>>>,
    calls: Arc<Mutex<u32>>,
}

impl Script {
    fn new(outcomes: Vec<Outcome>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(outcomes.into())),
            calls: Arc::new(Mutex::new(0)),
        }
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap()
    }

    fn fetch_fn(&self) -> Arc<FetchUsageFn> {
        let script = self.clone();
        Arc::new(move |_cookie| {
            let script = script.clone();
            Box::pin(async move {
                *script.calls.lock().unwrap() += 1;
                let mut q = script.outcomes.lock().unwrap();
                if q.len() > 1 {
                    q.pop_front().unwrap()
                } else {
                    q[0].clone()
                }
            })
        })
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap()
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(t0()).with_offset(FixedOffset::west_opt(6 * 3600).unwrap()))
}

fn window(used: f64, resets_at: Option<DateTime<Utc>>) -> ClaudeUsageWindow {
    ClaudeUsageWindow {
        used_percent: used,
        resets_at,
        scope_name: None,
    }
}

fn response(
    five: Option<f64>,
    weekly: Option<(f64, Option<DateTime<Utc>>)>,
) -> ClaudeUsageResponse {
    ClaudeUsageResponse {
        five_hour: five.map(|p| window(p, None)),
        seven_day: weekly.map(|(p, r)| window(p, r)),
    }
}

fn code_summary() -> ClaudeCodeUsageSummary {
    ClaudeCodeUsageSummary {
        month: UsageTotals {
            tokens: 10,
            cost_usd: 1.0,
            has_unpriced: false,
        },
        all_time: UsageTotals {
            tokens: 20,
            cost_usd: 2.0,
            has_unpriced: false,
        },
        month_models: Vec::new(),
    }
}

fn loader(summary: ClaudeCodeUsageSummary) -> Arc<LoadCodeUsageFn> {
    Arc::new(move |_now| {
        let summary = summary.clone();
        Box::pin(async move {
            Some(ClaudeCodeUsageScan {
                summary,
                local_daily_tokens: std::collections::BTreeMap::default(),
            })
        })
    })
}

struct Rig {
    poller: Arc<ClaudeUsagePoller>,
    auth: Arc<FakeSession>,
    clock: Arc<ManualClock>,
    daily_text: Arc<InMemoryStringStore>,
}

impl Rig {
    /// A second view of the persisted daily store (what the poller wrote).
    fn daily_view(&self) -> DailyQuotaDeltaStore {
        DailyQuotaDeltaStore::new(
            self.daily_text.clone(),
            "claude_daily_usage",
            self.clock.clone(),
        )
    }
}

struct RigBuilder {
    auth: Arc<FakeSession>,
    fetch: Arc<FetchUsageFn>,
    load: Arc<LoadCodeUsageFn>,
    daily_text: Arc<InMemoryStringStore>,
    clock: Arc<ManualClock>,
}

impl RigBuilder {
    fn new(fetch: Arc<FetchUsageFn>) -> Self {
        Self {
            auth: FakeSession::signed_in("sessionKey=live; lastActiveOrg=org"),
            fetch,
            load: no_code_usage(),
            daily_text: Arc::new(InMemoryStringStore::default()),
            clock: clock(),
        }
    }

    fn scripted(outcomes: Vec<Outcome>) -> Self {
        Self::new(Script::new(outcomes).fetch_fn())
    }

    fn auth(mut self, auth: Arc<FakeSession>) -> Self {
        self.auth = auth;
        self
    }

    fn load(mut self, load: Arc<LoadCodeUsageFn>) -> Self {
        self.load = load;
        self
    }

    fn daily_text(mut self, text: Arc<InMemoryStringStore>) -> Self {
        self.daily_text = text;
        self
    }

    fn build(self) -> Rig {
        let file = Arc::new(SettingsFile::in_memory());
        let settings = Arc::new(AppSettings::new(file));
        settings.set_selected_provider(ProviderId::Claude);
        let hourly = HourlyDeltaActivityStore::new(
            Arc::new(InMemoryStringStore::default()),
            "claude_hourly",
            self.clock.clone(),
        );
        let daily = DailyQuotaDeltaStore::new(
            self.daily_text.clone(),
            "claude_daily_usage",
            self.clock.clone(),
        );
        let poller = ClaudeUsagePoller::new(ClaudePollerDeps {
            settings,
            auth: self.auth.clone(),
            hourly,
            daily,
            clock: self.clock.clone(),
            fetch_usage: self.fetch,
            load_code_usage: self.load,
        });
        Rig {
            poller,
            auth: self.auth,
            clock: self.clock,
            daily_text: self.daily_text,
        }
    }
}

fn unauthorized() -> Outcome {
    Err(ProviderError::Unauthorized(CTX))
}

// ------------------------------------------------- bars and store recording

fn local_at(clock: &ManualClock, day_offset: i64, hour: i64) -> DateTime<Utc> {
    let cal = Calendar::local(clock);
    let day = cal
        .add_days(cal.start_of_day(clock.now()), day_offset)
        .unwrap();
    day + chrono::Duration::hours(hour)
}

struct StoreRig {
    clock: Arc<ManualClock>,
    store: DailyQuotaDeltaStore,
}

impl StoreRig {
    fn new() -> Self {
        let clock = clock();
        let store = DailyQuotaDeltaStore::new(
            Arc::new(InMemoryStringStore::default()),
            "claude_daily_usage",
            clock.clone(),
        );
        Self { clock, store }
    }

    fn poll(
        &mut self,
        used: f64,
        fetched: DateTime<Utc>,
        reset_offset: i64,
    ) -> Vec<DailyBudgetDay> {
        let resets_at = local_at(&self.clock, reset_offset, 8);
        self.store.record(
            used,
            fetched,
            Some(QuotaWindow {
                start: None,
                resets_at: Some(resets_at),
            }),
            7,
        );
        let spent = self.store.spent_by_day().into_iter().collect();
        build_daily_budget_days(
            &spent,
            Some(resets_at),
            self.store.window_start(),
            self.store.interrupted_window_start(),
            WeekOffset::CURRENT,
            fetched,
            &Calendar::local(self.clock.as_ref()),
        )
    }
}

/// Claude sends only `seven_day.resets_at`. A drop plus a reset instant that
/// jumps while the old one is still ahead is an early reset: prior days stay
/// as dimmed history and the 7 live bars open on the new window's start.
#[test]
fn test_early_reset_keeps_prior_days_and_restarts_the_weekly_window() {
    let mut rig = StoreRig::new();
    let c = rig.clock.clone();
    rig.poll(20.0, local_at(&c, -2, 9), 5);
    rig.poll(45.0, local_at(&c, -1, 9), 5);

    let days = rig.poll(2.0, local_at(&c, 0, 9), 7);

    let live: Vec<_> = days.iter().filter(|d| !d.is_prior_window).collect();
    let prior: Vec<_> = days.iter().filter(|d| d.is_prior_window).collect();
    let cal = Calendar::local(c.as_ref());
    assert_eq!(live.len(), 7);
    assert_eq!(cal.date_of(live[0].date), cal.date_of(local_at(&c, 0, 8)));
    assert!((live[0].spent_usd - 2.0).abs() < 1e-3);
    assert!((prior.last().unwrap().spent_usd - 25.0).abs() < 1e-3);
    assert_eq!(
        DailyBudget::weekly_pace_period_start(&days, &cal),
        Some(cal.start_of_day(local_at(&c, 0, 8)))
    );
}

/// Without any reset movement a bigger-than-noise drop is still just the
/// store's drop-as-reset credit; no prior-window bars appear.
#[test]
fn test_same_reset_instant_drop_leaves_bars_unchanged() {
    let mut rig = StoreRig::new();
    let c = rig.clock.clone();
    rig.poll(60.0, local_at(&c, -1, 9), 5);
    let days = rig.poll(10.0, local_at(&c, 0, 9), 5);
    assert_eq!(days.len(), 7);
    assert!(days.iter().all(|d| !d.is_prior_window));
}

/// `seven_day: null` with the weekly pool in `limits` records the daily store
/// and publishes seven bars.
#[tokio::test]
async fn test_refresh_builds_bars_from_weekly_limit_when_seven_day_is_null() {
    let payload = br#"{"five_hour": {"utilization": 14}, "seven_day": null,
        "limits": [{"group": "weekly", "percent": 3, "resets_at": "2099-01-04T08:00:00+00:00",
                    "scope": {"model": {"display_name": "Fable"}}}]}"#;
    let parsed = parse_usage_body(payload).unwrap();
    let rig = RigBuilder::scripted(vec![Ok((parsed, t0()))]).build();

    rig.poller.refresh_now().await;

    assert_eq!(rig.poller.daily_budget_days().unwrap().len(), 7);
    let store = rig.daily_view();
    assert!((store.last_used_percent().unwrap() - 3.0).abs() < 1e-3);
    assert!(store.window_resets_at().is_some());
}

/// A weekly pool without `resets_at` reuses the reset the store persisted on
/// an earlier run, carried forward to the current week.
#[tokio::test]
async fn test_refresh_without_reset_time_reuses_persisted_reset() {
    let text = Arc::new(InMemoryStringStore::default());
    let earlier = t0() - chrono::Duration::days(10);
    let mut seed = DailyQuotaDeltaStore::new(text.clone(), "claude_daily_usage", clock());
    seed.record(
        5.0,
        earlier,
        Some(QuotaWindow {
            start: None,
            resets_at: Some(earlier + chrono::Duration::days(1)),
        }),
        7,
    );
    let rig = RigBuilder::scripted(vec![Ok((response(None, Some((9.0, None))), t0()))])
        .daily_text(text)
        .build();

    rig.poller.refresh_now().await;

    assert_eq!(rig.poller.daily_budget_days().unwrap().len(), 7);
    let reset = rig.poller.weekly_resets_at(t0()).unwrap();
    assert!(reset > t0());
    assert!(reset <= t0() + chrono::Duration::days(7));
}

/// Earlier weeks are browsable from the live store without a new poll.
#[tokio::test]
async fn test_previous_week_bars_come_from_the_live_store() {
    let reset = t0() + chrono::Duration::days(3);
    let rig =
        RigBuilder::scripted(vec![Ok((response(None, Some((9.0, Some(reset)))), t0()))]).build();
    rig.poller.refresh_now().await;

    let current = rig.poller.daily_budget_days_at(WeekOffset::CURRENT);
    let previous = rig
        .poller
        .daily_budget_days_at(WeekOffset::CURRENT.previous());

    assert_eq!(current, rig.poller.daily_budget_days().unwrap());
    assert_eq!(previous.len(), 7);
    assert_ne!(previous[0].date, current[0].date);
}

// ------------------------------------------------------ Claude Code usage

/// Each refresh publishes the Claude Code usage loaded from local logs, and a
/// poller built with `no_code_usage` reads no local logs.
#[tokio::test]
async fn test_refresh_loads_claude_code_usage() {
    let ok = || vec![Ok((response(Some(1.0), None), t0()))];
    let with = RigBuilder::scripted(ok())
        .load(loader(code_summary()))
        .build();
    with.poller.refresh_now().await;
    assert!(with.poller.snapshot().is_some());
    assert_eq!(with.poller.code_usage(), Some(code_summary()));

    let hermetic = RigBuilder::scripted(ok()).build();
    hermetic.poller.refresh_now().await;
    assert!(hermetic.poller.snapshot().is_some());
    assert_eq!(hermetic.poller.code_usage(), None);
}

/// Claude Code usage comes from this machine's logs, so it loads without a
/// claude.ai session and when the claude.ai fetch fails.
#[tokio::test]
async fn test_claude_code_usage_loads_without_session_and_on_fetch_failure() {
    let failing = RigBuilder::scripted(vec![Err(ProviderError::Network(CTX, "offline".into()))])
        .load(loader(code_summary()))
        .build();
    failing.poller.refresh_now().await;
    assert!(failing.poller.snapshot().is_none());
    assert_eq!(failing.poller.code_usage(), Some(code_summary()));
    assert!(failing.poller.last_error().is_some());

    let script = Script::new(vec![Ok((response(Some(1.0), None), t0()))]);
    let signed_out = RigBuilder::new(script.fetch_fn())
        .auth(Arc::new(FakeSession::default()))
        .load(loader(code_summary()))
        .build();
    signed_out.poller.refresh_now().await;
    assert_eq!(script.calls(), 0, "no fetch without a session");
    assert!(signed_out.poller.snapshot().is_none());
    assert_eq!(signed_out.poller.code_usage(), Some(code_summary()));
    assert!(matches!(
        signed_out.poller.last_error(),
        Some(ProviderError::NotSignedIn(_))
    ));
    assert!(signed_out.auth.state().needs_sign_in);
}

/// Sign-out drops the claude.ai snapshot but keeps the Claude Code usage.
#[tokio::test]
async fn test_sign_out_keeps_claude_code_usage() {
    let rig = RigBuilder::scripted(vec![Ok((response(Some(1.0), None), t0()))])
        .load(loader(code_summary()))
        .build();
    rig.poller.refresh_now().await;

    rig.poller.clear_snapshot();

    assert!(rig.poller.snapshot().is_none());
    assert_eq!(rig.poller.code_usage(), Some(code_summary()));
}

// ------------------------------------------------- session guard (stale/live)

/// A 401 that finishes after the session was replaced must not invalidate
/// the new session.
#[tokio::test]
async fn test_claude_stale_unauthorized_does_not_invalidate_new_session() {
    let auth = FakeSession::signed_in("sessionKey=old; lastActiveOrg=org");
    let swapper = auth.clone();
    let fetch: Arc<FetchUsageFn> = Arc::new(move |_| {
        let auth = swapper.clone();
        Box::pin(async move {
            auth.sign_out();
            auth.save("sessionKey=new; lastActiveOrg=org");
            Err(ProviderError::Unauthorized(CTX))
        })
    });
    let rig = RigBuilder::new(fetch).auth(auth.clone()).build();

    rig.poller.refresh_now().await;

    let state = auth.state();
    assert!(state.is_signed_in);
    assert!(!state.needs_sign_in);
    assert_eq!(state.consecutive_auth_failures, 0);
}

/// Repeated 401s on the live session invalidate it after the threshold.
#[tokio::test]
async fn test_claude_current_unauthorized_invalidates_session() {
    let rig = RigBuilder::scripted(vec![unauthorized()]).build();
    for _ in 0..AUTH_FAILURE_THRESHOLD {
        rig.poller.refresh_now().await;
    }
    let state = rig.auth.state();
    assert!(state.needs_sign_in);
    assert!(!state.is_signed_in);
}

// ------------------------------------------------------ auth invalidation

fn invalidation_rig() -> Rig {
    let now = t0();
    RigBuilder::scripted(vec![
        Ok((
            response(
                Some(10.0),
                Some((10.0, Some(now + chrono::Duration::days(3)))),
            ),
            now - chrono::Duration::seconds(60),
        )),
        Ok((
            response(
                Some(10.0),
                Some((20.0, Some(now + chrono::Duration::days(3)))),
            ),
            now,
        )),
        unauthorized(),
    ])
    .build()
}

/// An expired Claude session asks for sign-in but keeps the snapshot and the
/// persisted daily history.
#[tokio::test]
async fn test_claude_invalidation_keeps_history() {
    let rig = invalidation_rig();
    rig.poller.refresh_now().await;
    rig.poller.refresh_now().await;
    assert!(!rig.daily_view().spent_by_day().is_empty());

    for _ in 0..AUTH_FAILURE_THRESHOLD {
        rig.poller.refresh_now().await;
    }

    let state = rig.auth.state();
    assert!(state.needs_sign_in);
    assert!(!state.is_signed_in);
    assert!(rig.poller.snapshot().is_some());
    assert!(!rig.daily_view().spent_by_day().is_empty());
}

/// A failed refresh after a successful one reports its error alongside the
/// kept snapshot, so the panel marks the data as stale.
#[tokio::test]
async fn test_claude_failure_after_data_reports_error() {
    let rig = invalidation_rig();
    rig.poller.refresh_now().await;
    rig.poller.refresh_now().await;
    assert!(rig.poller.last_error().is_none());

    rig.poller.refresh_now().await;

    assert!(rig.poller.snapshot().is_some());
    assert_eq!(
        rig.poller.last_error(),
        Some(ProviderError::Unauthorized(CTX))
    );
}

/// Sign-out drops the snapshot but keeps the daily history for the same
/// account; a different account's sign-in clears it.
#[tokio::test]
async fn test_claude_sign_out_keeps_history_until_another_account_signs_in() {
    let rig = invalidation_rig();
    rig.auth.adopt("org-1");
    rig.poller.refresh_now().await;
    rig.poller.refresh_now().await;
    let recorded = rig.daily_view().spent_by_day();
    assert!(!recorded.is_empty());

    rig.auth.sign_out();
    assert!(rig.poller.snapshot().is_none());
    assert_eq!(rig.daily_view().spent_by_day(), recorded);

    rig.auth.adopt("org-1");
    assert_eq!(rig.daily_view().spent_by_day(), recorded);

    rig.auth.sign_out();
    rig.auth.adopt("org-2");
    assert!(rig.daily_view().spent_by_day().is_empty());
}

// ------------------------------------------------------- loop integration

/// A disabled provider is skipped before anything is fetched or loaded.
#[tokio::test]
async fn test_disabled_provider_is_not_refreshed() {
    let script = Script::new(vec![Ok((response(Some(1.0), None), t0()))]);
    let rig = RigBuilder::new(script.fetch_fn())
        .load(loader(code_summary()))
        .build();
    let settings = Arc::new(AppSettings::new(Arc::new(SettingsFile::in_memory())));
    let enabled = [ProviderId::Grok].into_iter().collect();
    settings.set_enabled_providers(&enabled).unwrap();
    // A second poller sharing nothing with `rig`, built over the disabled settings.
    let poller = ClaudeUsagePoller::new(ClaudePollerDeps {
        settings,
        auth: rig.auth.clone(),
        hourly: HourlyDeltaActivityStore::new(
            Arc::new(InMemoryStringStore::default()),
            "h",
            rig.clock.clone(),
        ),
        daily: DailyQuotaDeltaStore::new(
            Arc::new(InMemoryStringStore::default()),
            "d",
            rig.clock.clone(),
        ),
        clock: rig.clock.clone(),
        fetch_usage: script.fetch_fn(),
        load_code_usage: loader(code_summary()),
    });

    poller.refresh_now().await;

    assert_eq!(script.calls(), 0);
    assert!(poller.code_usage().is_none());
}

/// `start` refreshes at once, then waits the idle interval; opening the menu
/// wakes the loop so the shorter active interval applies immediately.
#[tokio::test]
async fn test_start_polls_and_menu_open_wakes_the_loop() {
    let script = Script::new(vec![Ok((response(Some(1.0), None), t0()))]);
    let sched = ManualScheduler::new();
    let file = Arc::new(SettingsFile::in_memory());
    let settings = Arc::new(AppSettings::new(file));
    settings.set_selected_provider(ProviderId::Claude);
    let c = sched.clock();
    let poller = ClaudeUsagePoller::with_loop(
        ClaudePollerDeps {
            settings: settings.clone(),
            auth: FakeSession::signed_in("sessionKey=live; lastActiveOrg=org"),
            hourly: HourlyDeltaActivityStore::new(
                Arc::new(InMemoryStringStore::default()),
                "h",
                c.clone(),
            ),
            daily: DailyQuotaDeltaStore::new(
                Arc::new(InMemoryStringStore::default()),
                "d",
                c.clone(),
            ),
            clock: c.clone(),
            fetch_usage: script.fetch_fn(),
            load_code_usage: no_code_usage(),
        },
        |b| b.sleeper(Arc::new(sched.clone())),
    );

    poller.start();
    let idle = sched.next_sleep().await;
    assert_eq!(script.calls(), 1);
    // The loop jitters each wait by +/- 10%.
    let within = |d: Duration, secs: u32| {
        let want = f64::from(secs);
        (want * 0.9..=want * 1.1).contains(&d.as_secs_f64())
    };
    let s = settings.snapshot();
    assert!(within(idle, s.idle_poll_seconds), "idle wait was {idle:?}");

    poller.set_menu_open(true);
    let active = sched.next_sleep().await;
    assert!(
        within(active, s.active_poll_seconds),
        "active wait was {active:?}"
    );
    poller.stop();
}

/// The trait-level headline values mirror the snapshot.
#[tokio::test]
async fn test_headline_values_follow_the_snapshot() {
    let reset = t0() + chrono::Duration::hours(2);
    let mut r = response(Some(42.0), None);
    r.five_hour.as_mut().unwrap().resets_at = Some(reset);
    let rig = RigBuilder::scripted(vec![Ok((r, t0()))]).build();
    assert_eq!(rig.poller.headline_used_percent(), None);

    rig.poller.refresh_now().await;

    assert_eq!(rig.poller.provider(), ProviderId::Claude);
    assert_eq!(rig.poller.headline_used_percent(), Some(42.0));
    assert_eq!(rig.poller.headline_resets_at(), Some(reset));
    assert_eq!(rig.poller.last_refreshed_at(), Some(t0()));
}
