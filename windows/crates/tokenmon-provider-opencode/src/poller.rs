//! `OpenCodeUsagePoller`: the console poll with a local-estimate fallback,
//! driven by core's [`PollingLoop`] (spec 03 sections 3.10 and 3.11).
//!
//! One refresh tries the console first (when a cookie exists) through
//! [`ProviderRefresh::run`], which owns the stale-session guard and the
//! auth-rejection strikes. Whenever the console did not publish, the local
//! `opencode.db` estimate is published instead with a caption saying why, so
//! the panel always has numbers. A failed console fetch still returns a
//! failure outcome so the loop backs off.
//!
//! The polling loop's closures hold only a [`Weak`] reference to the poller.
//!
//! # Locking
//!
//! One `state` mutex; no lock is held across an `.await`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, PollingLoopBuilder, ProviderRefresh};
use tokenmon_core::settings::AppSettings;
use tokenmon_core::usage::{Calendar, DailyBudgetDay, DailyTokenUsage, DayKey, WeekOffset};
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, BoxFuture, Clock, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};

use crate::budget::{
    MonthBudgetRequest, OpenCodeMonthBudget, ScanFn, daily_tokens, month_daily_budget_days,
    with_tokens,
};
use crate::captions;
use crate::console::{ConsoleClient, ConsoleTransport};
use crate::db::{DataDirEnv, ScanCache, load_scan, locate_db, scan_start};
use crate::snapshot::{OpenCodeSnapshot, WindowKind};
use crate::stats::{DayHourlyUsage, build_day_hourly_usage, snapshot as local_snapshot};

/// Legend size of the hourly stacks.
const MAX_LEGEND_MODELS: usize = 8;

/// Monthly limit used when the snapshot has no monthly window.
const DEFAULT_MONTHLY_LIMIT_USD: f64 = 60.0;

/// What the poller needs from the OpenCode session beyond [`AuthSession`]:
/// the remembered workspace and the "show the sign-in prompt" flag.
pub trait OpenCodeSession: AuthSession {
    /// The workspace id remembered from the last good fetch.
    fn workspace_id(&self) -> Option<String>;
    /// Remembers the workspace id.
    fn set_workspace_id(&self, id: &str);
    /// Sets the "show the sign-in prompt" flag.
    fn set_needs_sign_in(&self, needs: bool);
}

/// Console seam: takes the cookie header and the known workspace id, returns
/// the official snapshot and the workspace id that served it. Build the live
/// one with [`console_fetch_fn`].
pub type FetchConsoleFn = dyn Fn(
        String,
        Option<String>,
    ) -> BoxFuture<'static, Result<(OpenCodeSnapshot, String), ProviderError>>
    + Send
    + Sync;

/// A console seam over a [`ConsoleClient`].
pub fn console_fetch_fn<T: ConsoleTransport + 'static>(
    client: Arc<ConsoleClient<T>>,
) -> Arc<FetchConsoleFn> {
    Arc::new(move |cookie, org| {
        let client = Arc::clone(&client);
        Box::pin(async move {
            client
                .fetch_go_usage_snapshot(&cookie, org.as_deref())
                .await
        })
    })
}

/// The live scan seam: finds `opencode.db` through `env` on every call and
/// reads it read-only through a fingerprint cache that lives as long as the
/// seam.
pub fn live_scan_source(env: DataDirEnv) -> Arc<ScanFn> {
    let cache = ScanCache::default();
    Arc::new(move |since| {
        let path = locate_db(&env, std::path::Path::is_file).ok_or_else(|| {
            crate::db::DbError::Missing("opencode.db (no data directory known)".to_owned())
        })?;
        load_scan(&path, since, &cache)
    })
}

/// Everything an [`OpenCodeUsagePoller`] is built from.
pub struct OpenCodePollerDeps {
    /// App settings: provider enablement and poll intervals.
    pub settings: Arc<AppSettings>,
    /// The console cookie session.
    pub auth: Arc<dyn OpenCodeSession>,
    /// Time source for stamps and the local calendar.
    pub clock: Arc<dyn Clock>,
    /// The console fetch.
    pub fetch_console: Arc<FetchConsoleFn>,
    /// The local database scan.
    pub load_scan: Arc<ScanFn>,
}

/// Bars plus the tokens attached to them, kept so earlier weeks can be
/// browsed without reading the database again.
#[derive(Debug, Clone)]
struct BudgetSource {
    budget: OpenCodeMonthBudget,
    tokens: HashMap<DayKey, DailyTokenUsage>,
}

/// What one local read produced.
struct Computed {
    snapshot: Option<OpenCodeSnapshot>,
    hourly: Option<DayHourlyUsage>,
    budget: Option<BudgetSource>,
}

#[derive(Default)]
struct State {
    snapshot: Option<OpenCodeSnapshot>,
    day_hourly_usage: Option<DayHourlyUsage>,
    budget: Option<BudgetSource>,
    last_error_text: Option<String>,
    last_refreshed_at: Option<DateTime<Utc>>,
    data_source_label: Option<&'static str>,
}

/// Clears the in-flight flag on drop.
struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Forwards session events to the poller without keeping it alive.
struct SessionListener(Weak<OpenCodeUsagePoller>);

impl AuthEventListener for SessionListener {
    fn on_auth_event(&self, _event: AuthEvent) {
        if let Some(poller) = self.0.upgrade() {
            poller.clear_snapshot();
        }
    }
}

/// Polls the OpenCode console and falls back to the local database estimate.
pub struct OpenCodeUsagePoller {
    settings: Arc<AppSettings>,
    auth: Arc<dyn OpenCodeSession>,
    clock: Arc<dyn Clock>,
    fetch_console: Arc<FetchConsoleFn>,
    load_scan: Arc<ScanFn>,
    state: Mutex<State>,
    menu_open: AtomicBool,
    refreshing: AtomicBool,
    polling: Arc<PollingLoop>,
}

impl OpenCodeUsagePoller {
    /// Builds the poller and its (not yet started) polling loop, and registers
    /// for the session's sign-out and account-reset events.
    ///
    /// # Panics
    ///
    /// Outside a tokio runtime (the loop captures the current handle).
    pub fn new(deps: OpenCodePollerDeps) -> Arc<Self> {
        Self::with_loop(deps, |builder| builder)
    }

    /// Like [`OpenCodeUsagePoller::new`], letting `configure` adjust the loop
    /// builder (sleeper, jitter, runtime handle). The poller's clock is
    /// already set on the builder.
    ///
    /// # Panics
    ///
    /// Outside a tokio runtime unless `configure` sets a handle.
    pub fn with_loop(
        deps: OpenCodePollerDeps,
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
                fetch_console: deps.fetch_console,
                load_scan: deps.load_scan,
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
            .poll_interval(ProviderId::OpenCode, self.menu_open.load(Ordering::SeqCst))
    }

    /// The displayed snapshot (console or local estimate).
    pub fn snapshot(&self) -> Option<OpenCodeSnapshot> {
        self.state().snapshot.clone()
    }

    /// Today's hourly model stacks from the local database.
    pub fn day_hourly_usage(&self) -> Option<DayHourlyUsage> {
        self.state().day_hourly_usage.clone()
    }

    /// Label of the data behind the snapshot: `OpenCode console` or
    /// `Local estimate`.
    pub fn data_source_label(&self) -> Option<&'static str> {
        self.state().data_source_label
    }

    /// The exact caption of the last problem (`lastError` text), if any.
    pub fn last_error_text(&self) -> Option<String> {
        self.state().last_error_text.clone()
    }

    /// Whether a refresh is running.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::SeqCst)
    }

    /// Start of the first day of the billing period behind the bars.
    pub fn daily_budget_period_start(&self) -> Option<DateTime<Utc>> {
        self.state().budget.as_ref().map(|b| b.budget.period_start)
    }

    /// Bars of the current week of the subscription month.
    pub fn daily_budget_days(&self) -> Option<Vec<DailyBudgetDay>> {
        self.daily_budget_days_at(WeekOffset::CURRENT)
    }

    /// Bars for the week `week_offset` steps back, rebuilt from the stored
    /// percents with the tokens attached (no new database read).
    pub fn daily_budget_days_at(&self, week_offset: WeekOffset) -> Option<Vec<DailyBudgetDay>> {
        let state = self.state();
        let source = state.budget.as_ref()?;
        let cal = Calendar::local(self.clock.as_ref());
        let days = source.budget.days_at(week_offset, &cal)?;
        Some(with_tokens(days, &source.tokens, &cal))
    }

    /// Drops the snapshot, bars, hourly stacks, source label and error.
    pub fn clear_snapshot(&self) {
        *self.state() = State::default();
    }

    /// Loads the scan and builds the local estimate, hourly stacks and bars.
    /// With a console snapshot the local models and monthly totals are merged
    /// into it; a failed scan then leaves the console snapshot as is.
    async fn compute(&self, now: DateTime<Utc>, console: Option<OpenCodeSnapshot>) -> Computed {
        let load = Arc::clone(&self.load_scan);
        let clock = Arc::clone(&self.clock);
        tokio::task::spawn_blocking(move || {
            let cal = Calendar::local(clock.as_ref());
            let scan = load(scan_start(now)).ok();
            let local = scan.as_ref().map(|s| local_snapshot(s, now));
            let snapshot = match console {
                Some(c) => Some(c.merged_with_local(local.as_ref())),
                None => local,
            };
            let hourly = scan.as_ref().map(|s| {
                build_day_hourly_usage(&s.rows, now, cal.offset_at(now), MAX_LEGEND_MODELS)
            });
            let budget = snapshot
                .as_ref()
                .and_then(|snap| build_budget(snap, now, load.as_ref(), &cal));
            Computed {
                snapshot,
                hourly,
                budget,
            }
        })
        .await
        .unwrap_or(Computed {
            snapshot: None,
            hourly: None,
            budget: None,
        })
    }

    /// One refresh: the console when signed in, then the local fallback.
    async fn perform_refresh(&self) -> PollOutcome {
        if !self
            .settings
            .snapshot()
            .is_provider_enabled(ProviderId::OpenCode)
            || self.refreshing.swap(true, Ordering::SeqCst)
        {
            return PollOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let cookie = self.auth.cookie_header().filter(|c| !c.is_empty());
        let has_cookie = cookie.is_some();
        let console_outcome = match cookie {
            Some(cookie) => match self.console_step(cookie).await {
                outcome @ (PollOutcome::Success | PollOutcome::Skipped) => return outcome,
                failure => failure,
            },
            None => PollOutcome::Success,
        };
        self.local_fallback(has_cookie, console_outcome).await
    }

    /// The console path. `Success` means it published; `Skipped` means the
    /// session changed under it; anything else is a failure to fall back from.
    async fn console_step(&self, cookie: String) -> PollOutcome {
        let known_org = self.auth.workspace_id();
        ProviderRefresh::run(
            self.auth.as_ref(),
            "OpenCode",
            || async {
                let (console, org) = (self.fetch_console)(cookie, known_org.clone()).await?;
                let computed = self.compute(self.clock.now(), Some(console)).await;
                Ok::<_, ProviderError>((computed, org))
            },
            |(computed, org)| self.publish_console(computed, &org, known_org.as_deref()),
            || {},
            |_message| {},
        )
        .await
    }

    fn publish_console(
        &self,
        computed: Computed,
        org: &str,
        known_org: Option<&str>,
    ) -> PollOutcome {
        let Some(snapshot) = computed.snapshot else {
            return PollOutcome::Skipped;
        };
        if known_org != Some(org) {
            self.auth.set_workspace_id(org);
        }
        {
            let mut state = self.state();
            state.snapshot = Some(snapshot);
            if computed.hourly.is_some() {
                state.day_hourly_usage = computed.hourly;
            }
            state.budget = computed.budget;
            state.last_error_text = None;
            state.last_refreshed_at = Some(self.clock.now());
            state.data_source_label = Some(captions::SOURCE_CONSOLE);
        }
        self.auth.set_needs_sign_in(false);
        PollOutcome::Success
    }

    /// Publishes the local estimate with the caption saying why the console
    /// did not. Returns `console_outcome` so a failed console still backs off.
    async fn local_fallback(&self, has_cookie: bool, console_outcome: PollOutcome) -> PollOutcome {
        // Adopt the generation after any invalidation this poll caused, so it
        // still publishes the estimate; a later change (sign-out) drops it.
        let generation = self.auth.state().session_generation;
        let computed = self.compute(self.clock.now(), None).await;
        if self.auth.state().session_generation != generation {
            return PollOutcome::Skipped;
        }
        let mut state = self.state();
        if computed.hourly.is_some() {
            state.day_hourly_usage = computed.hourly;
        }
        let Some(snapshot) = computed.snapshot else {
            state.last_error_text = Some(captions::LOCAL_UNREADABLE.to_owned());
            return console_outcome;
        };
        state.snapshot = Some(snapshot);
        state.budget = computed.budget;
        state.data_source_label = Some(captions::SOURCE_LOCAL);
        state.last_error_text =
            Some(captions::fallback_error(has_cookie, self.auth.state().needs_sign_in).to_owned());
        state.last_refreshed_at = Some(self.clock.now());
        console_outcome
    }
}

/// Step "buildDailyBudgetDays" (spec 03 section 3.11): the monthly window's
/// limit and reset anchor the bars; tokens are attached for the hover label.
fn build_budget(
    snapshot: &OpenCodeSnapshot,
    now: DateTime<Utc>,
    load: &ScanFn,
    cal: &Calendar<'_>,
) -> Option<BudgetSource> {
    let monthly = snapshot.window(WindowKind::Monthly);
    let limit_usd = monthly.map_or(DEFAULT_MONTHLY_LIMIT_USD, |m| m.limit_usd);
    if limit_usd <= 0.0 {
        return None;
    }
    let request = MonthBudgetRequest {
        limit_usd,
        used_percent: snapshot.monthly_used_percent(),
        period_resets_at: monthly.and_then(|m| m.resets_at),
        week_offset: WeekOffset::CURRENT,
        now,
        spent_by_day: None,
    };
    let budget = month_daily_budget_days(&request, load, cal)?;
    let tokens = daily_tokens(load, scan_start(now), cal);
    Some(BudgetSource {
        budget: OpenCodeMonthBudget {
            days: with_tokens(budget.days.clone(), &tokens, cal),
            ..budget
        },
        tokens,
    })
}

impl ProviderUsagePoller for OpenCodeUsagePoller {
    fn provider(&self) -> ProviderId {
        ProviderId::OpenCode
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
        self.state()
            .last_error_text
            .clone()
            .map(|text| ProviderError::BadResponse(ProviderErrorContext::OpenCode, text))
    }

    fn headline_used_percent(&self) -> Option<f64> {
        self.state()
            .snapshot
            .as_ref()
            .map(OpenCodeSnapshot::monthly_used_percent)
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        let state = self.state();
        let snapshot = state.snapshot.as_ref()?;
        snapshot
            .window(WindowKind::Monthly)
            .or_else(|| snapshot.window(WindowKind::Weekly))
            .and_then(|w| w.resets_at)
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
