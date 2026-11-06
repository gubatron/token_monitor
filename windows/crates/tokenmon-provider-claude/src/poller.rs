//! `ClaudeUsagePoller`: the claude.ai poll, the weekly bars and the Claude Code
//! usage load, driven by core's [`PollingLoop`] (spec 03 section 1.6).
//!
//! One refresh does two independent things:
//!
//! 1. Loads Claude Code usage from the local transcripts. This needs no
//!    claude.ai session, so it runs when signed out and when the fetch fails,
//!    and survives sign-out.
//! 2. Fetches claude.ai usage through [`ProviderRefresh::run`] (stale-session
//!    guard, auth strikes) and, on success, publishes the snapshot, records
//!    the hourly and daily stores and rebuilds the weekly bars.
//!
//! The polling loop's refresh and interval closures hold only a [`Weak`]
//! reference to the poller, so dropping the poller drops the loop.
//!
//! # Locking
//!
//! `state` is taken before `hourly` / `daily`; no lock is held across an
//! `.await`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, PollingLoopBuilder, ProviderRefresh};
use tokenmon_core::settings::AppSettings;
use tokenmon_core::storage::{DailyQuotaDeltaStore, HourlyDeltaActivityStore, QuotaWindow};
use tokenmon_core::usage::{Calendar, DailyBudgetDay, WeekOffset};
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, BoxFuture, Clock, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};

use crate::budget::{
    DAYS_IN_PERIOD, SpentByDay, attach_tokens, build_daily_budget_days, weekly_resets_at,
};
use crate::cache::ClaudeCodeFileCache;
use crate::discovery::{ClaudeEnvironment, projects_directories, scan_daily_tokens};
use crate::fetch::FetchedUsage;
use crate::ledger::ClaudeCodeUsageLedger;
use crate::summary::ClaudeCodeUsageSummary;
use crate::tokens::DailyTokens;
use crate::usage::{ClaudeSnapshot, ClaudeUsageResponse};

/// The one session operation the poller needs beyond [`AuthSession`]
/// (Swift `auth.needsSignIn = ...`). Core's trait has no setter for it; the
/// real session implements this too.
pub trait ClaudeSession: AuthSession {
    /// Sets the "show the sign-in prompt" flag.
    fn set_needs_sign_in(&self, needs: bool);
}

/// Fetch seam: takes the stored cookie header, returns the parsed usage and
/// the instant it was taken. Build the live one with
/// [`crate::fetch::ClaudeUsageClient::into_fetch_fn`].
pub type FetchUsageFn =
    dyn Fn(String) -> BoxFuture<'static, Result<FetchedUsage, ProviderError>> + Send + Sync;

/// Claude Code usage seam: takes "now", returns the scan or `None` when
/// Claude Code has no logs on this machine.
pub type LoadCodeUsageFn =
    dyn Fn(DateTime<Utc>) -> BoxFuture<'static, Option<ClaudeCodeUsageScan>> + Send + Sync;

/// What one load of Claude Code's local logs produces.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCodeUsageScan {
    /// Month and all-time totals from the forever-ledger.
    pub summary: ClaudeCodeUsageSummary,
    /// Tokens per local day and model, for the daily bars' hover label.
    pub local_daily_tokens: DailyTokens,
}

/// A load seam that reads nothing (what a poller with an injected fetch uses
/// so tests never touch the real home directory).
pub fn no_code_usage() -> Arc<LoadCodeUsageFn> {
    Arc::new(|_| Box::pin(async { None }))
}

/// The live load seam: scans the transcripts under `env` (UTC days for the
/// ledger, local days for the bars), merges the UTC scan into the ledger in
/// `ledger_dir` and builds the summary. Runs on a blocking thread; the
/// transcript cache lives as long as the returned seam.
pub fn live_code_usage(
    env: ClaudeEnvironment,
    ledger_dir: &Path,
    clock: Arc<dyn Clock>,
) -> Arc<LoadCodeUsageFn> {
    let cache = Arc::new(ClaudeCodeFileCache::new());
    let ledger = Arc::new(ClaudeCodeUsageLedger::new(ledger_dir));
    let env = Arc::new(env);
    Arc::new(move |now| {
        let (cache, ledger, env, clock) = (
            Arc::clone(&cache),
            Arc::clone(&ledger),
            Arc::clone(&env),
            Arc::clone(&clock),
        );
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let utc = Calendar::utc();
                let local = Calendar::local(clock.as_ref());
                let scans = scan_daily_tokens(&projects_directories(&env), &[utc, local], &cache)?;
                let [utc_days, local_days]: [DailyTokens; 2] = scans.try_into().ok()?;
                Some(ClaudeCodeUsageScan {
                    summary: ClaudeCodeUsageSummary::make(&ledger.merge(&utc_days), now),
                    local_daily_tokens: local_days,
                })
            })
            .await
            .ok()
            .flatten()
        })
    })
}

/// Everything a [`ClaudeUsagePoller`] is built from.
pub struct ClaudePollerDeps {
    /// App settings: provider enablement and poll intervals.
    pub settings: Arc<AppSettings>,
    /// The claude.ai cookie session.
    pub auth: Arc<dyn ClaudeSession>,
    /// Hourly activity series (5-hour window).
    pub hourly: HourlyDeltaActivityStore,
    /// Daily weekly-pool deltas.
    pub daily: DailyQuotaDeltaStore,
    /// Time source for stamps and the local calendar.
    pub clock: Arc<dyn Clock>,
    /// The claude.ai fetch.
    pub fetch_usage: Arc<FetchUsageFn>,
    /// The Claude Code local-usage load.
    pub load_code_usage: Arc<LoadCodeUsageFn>,
}

#[derive(Default)]
struct State {
    snapshot: Option<ClaudeSnapshot>,
    last_error: Option<ProviderError>,
    last_refreshed_at: Option<DateTime<Utc>>,
    daily_budget_days: Option<Vec<DailyBudgetDay>>,
    code_usage: Option<ClaudeCodeUsageSummary>,
    local_daily_tokens: DailyTokens,
    /// Last observed weekly `resets_at`; anchors the chart when a later
    /// payload omits it.
    weekly_resets_at: Option<DateTime<Utc>>,
    /// Instant the current bars were built against, so earlier weeks shift
    /// from the same window.
    budget_reference_now: Option<DateTime<Utc>>,
}

/// Clears the in-flight flag on drop.
struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Forwards session events to the poller without keeping it alive.
struct SessionListener(Weak<ClaudeUsagePoller>);

impl AuthEventListener for SessionListener {
    fn on_auth_event(&self, event: AuthEvent) {
        let Some(poller) = self.0.upgrade() else {
            return;
        };
        // Sign-out drops the snapshot; the persisted hourly and daily history
        // is wiped only when a different account signs in.
        poller.clear_snapshot();
        if event == AuthEvent::AccountReset {
            poller.clear_account_history();
        }
    }
}

/// Polls Claude rate limits, builds the weekly-window daily bars and loads
/// Claude Code usage.
pub struct ClaudeUsagePoller {
    settings: Arc<AppSettings>,
    auth: Arc<dyn ClaudeSession>,
    clock: Arc<dyn Clock>,
    fetch_usage: Arc<FetchUsageFn>,
    load_code_usage: Arc<LoadCodeUsageFn>,
    hourly: Mutex<HourlyDeltaActivityStore>,
    daily: Mutex<DailyQuotaDeltaStore>,
    state: Mutex<State>,
    menu_open: AtomicBool,
    refreshing: AtomicBool,
    polling: Arc<PollingLoop>,
}

impl ClaudeUsagePoller {
    /// Builds the poller and its (not yet started) polling loop, and registers
    /// for the session's sign-out and account-reset events.
    ///
    /// # Panics
    ///
    /// Outside a tokio runtime (the loop captures the current handle).
    pub fn new(deps: ClaudePollerDeps) -> Arc<Self> {
        Self::with_loop(deps, |builder| builder)
    }

    /// Like [`ClaudeUsagePoller::new`], letting `configure` adjust the loop
    /// builder (sleeper, jitter, runtime handle) before it is built. The
    /// poller's clock is already set on the builder.
    ///
    /// # Panics
    ///
    /// Outside a tokio runtime unless `configure` sets a handle.
    pub fn with_loop(
        deps: ClaudePollerDeps,
        configure: impl FnOnce(PollingLoopBuilder) -> PollingLoopBuilder,
    ) -> Arc<Self> {
        let poller = Arc::new_cyclic(|weak: &Weak<Self>| {
            let for_interval = weak.clone();
            let for_refresh = weak.clone();
            let builder = PollingLoop::builder(
                move || for_interval.upgrade().and_then(|p| p.poll_interval()),
                move || {
                    let poller = for_refresh.upgrade();
                    async move {
                        match poller {
                            Some(p) => p.perform_refresh().await,
                            None => PollOutcome::Skipped,
                        }
                    }
                },
            )
            .clock(Arc::clone(&deps.clock));
            Self {
                settings: deps.settings,
                auth: deps.auth,
                clock: deps.clock,
                fetch_usage: deps.fetch_usage,
                load_code_usage: deps.load_code_usage,
                hourly: Mutex::new(deps.hourly),
                daily: Mutex::new(deps.daily),
                state: Mutex::new(State::default()),
                menu_open: AtomicBool::new(false),
                refreshing: AtomicBool::new(false),
                polling: configure(builder).build(),
            }
        });
        poller
            .auth
            .add_listener(Arc::new(SessionListener(Arc::downgrade(&poller))));
        poller
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn poll_interval(&self) -> Option<Duration> {
        self.settings
            .snapshot()
            .poll_interval(ProviderId::Claude, self.menu_open.load(Ordering::SeqCst))
    }

    /// The displayed claude.ai snapshot.
    pub fn snapshot(&self) -> Option<ClaudeSnapshot> {
        self.state().snapshot.clone()
    }

    /// Daily bars for the current weekly window: percent of the weekly pool
    /// burned per calendar day, anchored to the pool's real `resets_at`.
    /// `None` until the first successful refresh.
    pub fn daily_budget_days(&self) -> Option<Vec<DailyBudgetDay>> {
        self.state().daily_budget_days.clone()
    }

    /// Claude Code usage on this machine at API rates; `None` when it has no
    /// logs. Loaded on every poll and kept across sign-out.
    pub fn code_usage(&self) -> Option<ClaudeCodeUsageSummary> {
        self.state().code_usage.clone()
    }

    /// Claude Code tokens per local day and model (hover labels).
    pub fn local_daily_tokens(&self) -> DailyTokens {
        self.state().local_daily_tokens.clone()
    }

    /// Whether a refresh is running.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::SeqCst)
    }

    /// Drops the claude.ai snapshot, bars and error; keeps the Claude Code
    /// usage and the persisted history.
    pub fn clear_snapshot(&self) {
        let mut state = self.state();
        state.snapshot = None;
        state.daily_budget_days = None;
        state.weekly_resets_at = None;
        state.budget_reference_now = None;
        state.last_error = None;
        state.last_refreshed_at = None;
    }

    /// Clears the previous account's hourly and daily history.
    fn clear_account_history(&self) {
        let now = self.clock.now();
        self.hourly
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear(now);
        self.daily
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Weekly reset instant the bars and the reset caption anchor to: the
    /// latest payload's, else the last one seen this run, else the one the
    /// daily store persisted earlier, carried forward by whole weeks when it
    /// has passed.
    pub fn weekly_resets_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let state = self.state();
        self.weekly_resets_at_locked(&state, now)
    }

    fn weekly_resets_at_locked(&self, state: &State, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let live = state
            .snapshot
            .as_ref()
            .and_then(|s| s.seven_day.as_ref())
            .and_then(|w| w.resets_at);
        let remembered = state.weekly_resets_at.or_else(|| {
            self.daily
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .window_resets_at()
        });
        weekly_resets_at(live, remembered, now)
    }

    /// Bars for the weekly window `week_offset` steps before the current one,
    /// read from the live daily store so the panel can browse earlier weeks
    /// without a new poll. [`WeekOffset::CURRENT`] matches
    /// [`ClaudeUsagePoller::daily_budget_days`].
    pub fn daily_budget_days_at(&self, week_offset: WeekOffset) -> Vec<DailyBudgetDay> {
        let state = self.state();
        let now = state
            .budget_reference_now
            .unwrap_or_else(|| self.clock.now());
        self.build_bars(&state, now, week_offset)
    }

    fn build_bars(
        &self,
        state: &State,
        now: DateTime<Utc>,
        week_offset: WeekOffset,
    ) -> Vec<DailyBudgetDay> {
        let resets_at = self.weekly_resets_at_locked(state, now);
        let calendar = Calendar::local(self.clock.as_ref());
        let daily = self.daily.lock().unwrap_or_else(PoisonError::into_inner);
        let spent: SpentByDay = daily.spent_by_day().into_iter().collect::<HashMap<_, _>>();
        let days = build_daily_budget_days(
            &spent,
            resets_at,
            daily.window_start(),
            daily.interrupted_window_start(),
            week_offset,
            now,
            &calendar,
        );
        attach_tokens(days, &state.local_daily_tokens, &calendar)
    }

    /// One refresh: the Claude Code load, then the claude.ai fetch.
    async fn perform_refresh(&self) -> PollOutcome {
        if !self
            .settings
            .snapshot()
            .is_provider_enabled(ProviderId::Claude)
            || self.refreshing.swap(true, Ordering::SeqCst)
        {
            return PollOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        self.load_code_usage().await;
        let Some(cookie) = self.auth.cookie_header().filter(|c| !c.is_empty()) else {
            self.auth.set_needs_sign_in(true);
            let mut state = self.state();
            if state.snapshot.is_none() {
                state.last_error = Some(ProviderError::NotSignedIn(ProviderErrorContext::Claude));
            }
            return PollOutcome::Skipped;
        };
        self.fetch_and_publish(cookie).await
    }

    async fn load_code_usage(&self) {
        let loaded = (self.load_code_usage)(self.clock.now()).await;
        let mut state = self.state();
        state.code_usage = loaded.as_ref().map(|l| l.summary.clone());
        state.local_daily_tokens = loaded.map(|l| l.local_daily_tokens).unwrap_or_default();
        if let Some(days) = state.daily_budget_days.take() {
            let calendar = Calendar::local(self.clock.as_ref());
            state.daily_budget_days =
                Some(attach_tokens(days, &state.local_daily_tokens, &calendar));
        }
    }

    async fn fetch_and_publish(&self, cookie: String) -> PollOutcome {
        // `ProviderRefresh` reports only the message; keep the typed error.
        let failure: Mutex<Option<ProviderError>> = Mutex::new(None);
        ProviderRefresh::run(
            self.auth.as_ref(),
            "Claude",
            || async {
                (self.fetch_usage)(cookie).await.inspect_err(|e| {
                    *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(e.clone());
                })
            },
            |fetched| self.publish(fetched),
            || {},
            |_message| {
                let error = failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                self.state().last_error = error;
            },
        )
        .await
    }

    fn publish(&self, (response, fetched_at): (ClaudeUsageResponse, DateTime<Utc>)) -> PollOutcome {
        let mut state = self.state();
        state.snapshot = Some(ClaudeSnapshot {
            fetched_at,
            five_hour: response.five_hour.clone(),
            seven_day: response.seven_day.clone(),
            account_email: self.auth.state().account_email,
        });
        state.last_error = None;
        state.last_refreshed_at = Some(self.clock.now());
        self.auth.set_needs_sign_in(false);
        self.record_stores(&mut state, &response, fetched_at);
        state.budget_reference_now = Some(fetched_at);
        state.daily_budget_days = Some(self.build_bars(&state, fetched_at, WeekOffset::CURRENT));
        PollOutcome::Success
    }

    /// The 5-hour window counts every model, unlike a model-scoped weekly
    /// limit, so it drives the hourly series. Claude sends only `resets_at`
    /// for the weekly pool, so a rollover or early reset is recognized by the
    /// daily store from a moved reset instant plus a used-% drop.
    fn record_stores(
        &self,
        state: &mut State,
        response: &ClaudeUsageResponse,
        fetched_at: DateTime<Utc>,
    ) {
        if let Some(window) = &response.five_hour {
            self.hourly
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .record(window.used_percent, fetched_at);
            tracing::info!("Claude refresh: 5h {:.1}% used", window.used_percent);
        }
        if let Some(window) = &response.seven_day {
            if let Some(resets_at) = window.resets_at {
                state.weekly_resets_at = Some(resets_at);
            }
            self.daily
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .record(
                    window.used_percent,
                    fetched_at,
                    Some(QuotaWindow {
                        start: None,
                        resets_at: window.resets_at,
                    }),
                    DAYS_IN_PERIOD,
                );
        }
    }
}

impl ProviderUsagePoller for ClaudeUsagePoller {
    fn provider(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn set_menu_open(&self, open: bool) {
        if self.menu_open.swap(open, Ordering::SeqCst) != open {
            self.polling.wake();
        }
    }

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        self.state().last_refreshed_at
    }

    fn last_error(&self) -> Option<ProviderError> {
        self.state().last_error.clone()
    }

    fn headline_used_percent(&self) -> Option<f64> {
        self.state()
            .snapshot
            .as_ref()
            .map(ClaudeSnapshot::headline_used_percent)
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        self.state()
            .snapshot
            .as_ref()
            .and_then(ClaudeSnapshot::resets_at)
    }

    fn alert_account(&self) -> Option<String> {
        self.auth.state().account_email
    }

    fn start(&self) {
        self.polling.start();
    }

    fn stop(&self) {
        self.polling.stop();
    }

    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.polling.refresh_now())
    }

    fn clear_snapshot(&self) {
        Self::clear_snapshot(self);
    }
}
