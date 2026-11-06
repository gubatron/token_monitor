//! `CursorUsagePoller` (spec 02 section 3.4): gating, the polling loop, the
//! publish step, the daily store and the Monday-week bars.
//!
//! The timing (interval, backoff, `Retry-After`, wake gate) is core's
//! [`PollingLoop`]; the fetch, stale-session and error step is core's
//! [`ProviderRefresh`]. This type owns what one refresh publishes:
//!
//! * the snapshot, hourly activity and carried-over event figures
//!   ([`carry_over`]);
//! * the day-over-day pool growth recorded in the [`DailyQuotaDeltaStore`]
//!   (which spots billing-cycle rollovers and early resets);
//! * the Monday-week bars ([`build_daily_budget_days`]) with each day's tokens.
//!
//! The refresh closures captured by the loop hold only a [`Weak`] reference to
//! the poller, so dropping the poller ends the loop.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, ProviderRefresh};
use tokenmon_core::storage::{DailyQuotaDeltaStore, QuotaWindow};
use tokenmon_core::usage::attaching_tokens;
use tokenmon_core::usage::{Calendar, DailyBudgetDay, DailyTokenUsage, WeekOffset};
use tokenmon_core::{
    AuthEvent, AuthEventListener, BoxFuture, Clock, ProviderAuthSession, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};

use crate::aggregate::CursorDayHourlyUsage;
use crate::bars::{DailyBudgetInputs, build_daily_budget_days};
use crate::client::{CursorFetchResult, CursorUsageClient};
use crate::models::CursorSnapshot;
use crate::publish::{PreviousPublish, account_email_changed, carry_over, cycle_length_days};

const CTX: ProviderErrorContext = ProviderErrorContext::Cursor;
/// Label shown under the panel's data.
pub const DATA_SOURCE_LABEL: &str = "Cursor dashboard";

/// One fetch with the given cookie header (live: [`CursorUsageClient`];
/// tests: a fake).
pub type FetchFn = Arc<
    dyn Fn(String) -> BoxFuture<'static, Result<CursorFetchResult, ProviderError>> + Send + Sync,
>;

/// Everything a [`CursorUsagePoller`] is wired to.
pub struct CursorPollerConfig {
    /// The shared Cursor session.
    pub session: Arc<dyn ProviderAuthSession>,
    /// Pool-percent growth per day (key `cursor_daily_usage` in production).
    pub daily: DailyQuotaDeltaStore,
    /// Local calendar and "now".
    pub clock: Arc<dyn Clock>,
    /// The fetch seam.
    pub fetch: FetchFn,
    /// Called on sign-out and account reset to drop cached event aggregates.
    pub clear_event_cache: Arc<dyn Fn() + Send + Sync>,
    /// `AppSettings.isProviderEnabled(.cursor)`.
    pub is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `AppSettings.pollInterval(for: .cursor, menuIsOpen:)`; `None` parks.
    pub poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
}

impl CursorPollerConfig {
    /// Config whose fetch is the live `client` (with its event cache).
    pub fn live(
        session: Arc<dyn ProviderAuthSession>,
        daily: DailyQuotaDeltaStore,
        clock: Arc<dyn Clock>,
        client: Arc<CursorUsageClient>,
        is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    ) -> Self {
        let fetch_client = Arc::clone(&client);
        let fetch_clock = Arc::clone(&clock);
        Self {
            session,
            daily,
            clock,
            fetch: Arc::new(move |cookie| {
                let client = Arc::clone(&fetch_client);
                let clock = Arc::clone(&fetch_clock);
                Box::pin(async move { client.fetch_snapshot(&cookie, clock.now()).await })
            }),
            clear_event_cache: Arc::new(move || client.cache().clear()),
            is_enabled,
            poll_interval,
        }
    }
}

/// Cycle bounds and weights captured at the last successful refresh, so the
/// panel can rebuild an earlier Monday week without paging events again.
#[derive(Debug, Clone)]
struct BudgetContext {
    estimated_weight_by_day: BTreeMap<NaiveDate, f64>,
    used_percent: f64,
    cycle_start: Option<DateTime<Utc>>,
    cycle_end: Option<DateTime<Utc>>,
    reference_now: DateTime<Utc>,
}

#[derive(Default)]
struct State {
    snapshot: Option<CursorSnapshot>,
    hourly: Option<CursorDayHourlyUsage>,
    daily_budget_days: Option<Vec<DailyBudgetDay>>,
    last_error: Option<ProviderError>,
    last_refreshed_at: Option<DateTime<Utc>>,
    data_source_label: Option<&'static str>,
    /// Last observed cycle end, so the chart stays anchored if a later payload
    /// omits it.
    billing_cycle_end: Option<DateTime<Utc>>,
    budget_context: Option<BudgetContext>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears `refreshing` when a refresh ends, however it ends.
struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct SessionEvents(Weak<CursorUsagePoller>);

impl AuthEventListener for SessionEvents {
    fn on_auth_event(&self, event: AuthEvent) {
        let Some(poller) = self.0.upgrade() else {
            return;
        };
        poller.clear_snapshot();
        if event == AuthEvent::AccountReset {
            lock(&poller.daily).clear();
        }
    }
}

/// Polls Cursor usage and builds billing-cycle daily bars.
pub struct CursorUsagePoller {
    session: Arc<dyn ProviderAuthSession>,
    daily: Mutex<DailyQuotaDeltaStore>,
    clock: Arc<dyn Clock>,
    fetch: FetchFn,
    clear_event_cache: Arc<dyn Fn() + Send + Sync>,
    is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    state: Mutex<State>,
    refreshing: AtomicBool,
    menu_open: AtomicBool,
    polling: Arc<PollingLoop>,
}

impl CursorUsagePoller {
    /// Wires the poller to its session and builds (but does not start) its
    /// polling loop.
    ///
    /// Signing out drops the snapshot; an account change also clears the
    /// daily history. The same-account history is kept across sign-outs.
    ///
    /// # Panics
    /// Outside a tokio runtime (the loop spawns its waits on it).
    pub fn new(config: CursorPollerConfig) -> Arc<Self> {
        let poller = Arc::new_cyclic(|weak: &Weak<Self>| {
            let for_interval = weak.clone();
            let for_refresh = weak.clone();
            let polling = PollingLoop::builder(
                move || for_interval.upgrade().and_then(|p| p.current_interval()),
                move || {
                    let weak = for_refresh.clone();
                    async move {
                        match weak.upgrade() {
                            Some(poller) => poller.perform_refresh().await,
                            None => PollOutcome::Skipped,
                        }
                    }
                },
            )
            .clock(Arc::clone(&config.clock))
            .build();
            Self {
                session: config.session,
                daily: Mutex::new(config.daily),
                clock: config.clock,
                fetch: config.fetch,
                clear_event_cache: config.clear_event_cache,
                is_enabled: config.is_enabled,
                poll_interval: config.poll_interval,
                state: Mutex::new(State::default()),
                refreshing: AtomicBool::new(false),
                menu_open: AtomicBool::new(false),
                polling,
            }
        });
        poller
            .session
            .add_listener(Arc::new(SessionEvents(Arc::downgrade(&poller))));
        poller
    }

    /// The polling loop (wake, pause and resume for the app shell).
    pub fn polling_loop(&self) -> &Arc<PollingLoop> {
        &self.polling
    }

    /// The last published snapshot.
    pub fn snapshot(&self) -> Option<CursorSnapshot> {
        lock(&self.state).snapshot.clone()
    }

    /// Today's hourly activity from the last refresh.
    pub fn day_hourly_usage(&self) -> Option<CursorDayHourlyUsage> {
        lock(&self.state).hourly.clone()
    }

    /// The Monday-week bars for the current week.
    pub fn daily_budget_days(&self) -> Option<Vec<DailyBudgetDay>> {
        lock(&self.state).daily_budget_days.clone()
    }

    /// Label under the panel's data, set by a successful refresh.
    pub fn data_source_label(&self) -> Option<&'static str> {
        lock(&self.state).data_source_label
    }

    /// Whether a refresh is in flight.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::SeqCst)
    }

    /// Whether the menu is open.
    pub fn is_menu_open(&self) -> bool {
        self.menu_open.load(Ordering::SeqCst)
    }

    /// Monday-week bars `week_offset` steps from the week containing the last
    /// refresh, rebuilt from the last refresh's inputs. `None` before the
    /// first refresh or when no billing month can be derived.
    pub fn daily_budget_days_at(&self, week_offset: WeekOffset) -> Option<Vec<DailyBudgetDay>> {
        let (context, tokens) = {
            let state = lock(&self.state);
            let tokens = state.snapshot.as_ref().map(|s| s.daily_tokens.clone());
            (state.budget_context.clone()?, tokens.unwrap_or_default())
        };
        let (observed, interrupted) = {
            let daily = lock(&self.daily);
            (daily.spent_by_day(), daily.interrupted_window_start())
        };
        let observed = observed.into_iter().collect();
        let days = build_daily_budget_days(
            self.clock.as_ref(),
            DailyBudgetInputs {
                observed_by_day: &observed,
                interrupted_window_start: interrupted,
                estimated_weight_by_day: &context.estimated_weight_by_day,
                used_percent: context.used_percent,
                cycle_start: context.cycle_start,
                cycle_end: context.cycle_end,
                week_offset,
                now: context.reference_now,
            },
        )?;
        Some(self.with_tokens(days, &tokens))
    }

    fn with_tokens(
        &self,
        days: Vec<DailyBudgetDay>,
        tokens: &BTreeMap<NaiveDate, DailyTokenUsage>,
    ) -> Vec<DailyBudgetDay> {
        let cal = Calendar::local(self.clock.as_ref());
        attaching_tokens(days, &cal, |key| {
            key.to_date().and_then(|date| tokens.get(&date).cloned())
        })
    }

    fn current_interval(&self) -> Option<Duration> {
        (self.poll_interval)(self.is_menu_open())
    }

    /// One refresh: gating, the guarded fetch and the publish step.
    ///
    /// Skips when the provider is disabled or a refresh is running. Without a
    /// cookie it flags `needs_sign_in` and skips.
    pub async fn perform_refresh(&self) -> PollOutcome {
        if !(self.is_enabled)() || self.refreshing.swap(true, Ordering::SeqCst) {
            return PollOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let Some(cookie) = self.session.cookie_header().filter(|c| !c.is_empty()) else {
            self.session.set_needs_sign_in(true);
            let mut state = lock(&self.state);
            if state.snapshot.is_none() {
                state.last_error = Some(ProviderError::NotSignedIn(CTX));
            }
            return PollOutcome::Skipped;
        };
        let failure: Mutex<Option<ProviderError>> = Mutex::new(None);
        ProviderRefresh::run(
            &*self.session,
            "Cursor",
            || async {
                let result = (self.fetch)(cookie).await;
                if let Err(error) = &result {
                    *lock(&failure) = Some(error.clone());
                }
                result
            },
            |fetched| self.publish(fetched),
            || {},
            |_message| lock(&self.state).last_error = lock(&failure).take(),
        )
        .await
    }

    fn publish(&self, fetched: CursorFetchResult) -> PollOutcome {
        // A different account fires `AccountReset`, which clears the previous
        // account's figures before this poll publishes. No lock is held here.
        if let Some(email) = fetched.snapshot.account_email.as_deref()
            && account_email_changed(Some(email), self.session.state().account_email.as_deref())
        {
            self.session.save_account_email(email);
        }
        let previous = self.previous_publish();
        let CursorFetchResult {
            snapshot,
            hourly,
            estimated_weight_by_day,
        } = carry_over(fetched, previous.as_ref());
        let cycle_end = snapshot
            .billing_cycle_end
            .or_else(|| lock(&self.state).billing_cycle_end);
        let (observed, interrupted) = self.record_daily(&snapshot, cycle_end);
        let observed = observed.into_iter().collect();
        let days = build_daily_budget_days(
            self.clock.as_ref(),
            DailyBudgetInputs {
                observed_by_day: &observed,
                interrupted_window_start: interrupted,
                estimated_weight_by_day: &estimated_weight_by_day,
                used_percent: snapshot.used_percent,
                cycle_start: snapshot.billing_cycle_start,
                cycle_end,
                week_offset: WeekOffset::CURRENT,
                now: snapshot.fetched_at,
            },
        )
        .map(|days| self.with_tokens(days, &snapshot.daily_tokens));
        tracing::info!(
            "Cursor refresh: total {:.1}% used ({:.0}% left)",
            snapshot.used_percent,
            (100.0 - snapshot.used_percent).round()
        );
        let context = BudgetContext {
            estimated_weight_by_day,
            used_percent: snapshot.used_percent,
            cycle_start: snapshot.billing_cycle_start,
            cycle_end,
            reference_now: snapshot.fetched_at,
        };
        {
            let mut state = lock(&self.state);
            state.billing_cycle_end = cycle_end;
            state.budget_context = Some(context);
            state.snapshot = Some(snapshot);
            state.hourly = Some(hourly);
            state.daily_budget_days = days;
            state.last_error = None;
            state.last_refreshed_at = Some(self.clock.now());
            state.data_source_label = Some(DATA_SOURCE_LABEL);
        }
        self.session.set_needs_sign_in(false);
        PollOutcome::Success
    }

    /// What carry-over needs from the previous publish, if there was one.
    fn previous_publish(&self) -> Option<PreviousPublish> {
        let state = lock(&self.state);
        Some(PreviousPublish {
            snapshot: state.snapshot.clone()?,
            hourly: state.hourly.clone()?,
            estimated_weight_by_day: state
                .budget_context
                .as_ref()
                .map(|c| c.estimated_weight_by_day.clone())
                .unwrap_or_default(),
        })
    }

    /// Records the pool percent in the daily store (which spots a rollover or
    /// an early reset from the cycle bounds) and returns what the bars need.
    fn record_daily(
        &self,
        snapshot: &CursorSnapshot,
        cycle_end: Option<DateTime<Utc>>,
    ) -> (BTreeMap<DateTime<Utc>, f64>, Option<DateTime<Utc>>) {
        let period_days =
            cycle_length_days(self.clock.as_ref(), snapshot.billing_cycle_start, cycle_end);
        let mut daily = lock(&self.daily);
        daily.record(
            snapshot.used_percent,
            snapshot.fetched_at,
            Some(QuotaWindow {
                start: snapshot.billing_cycle_start,
                resets_at: cycle_end,
            }),
            period_days,
        );
        (daily.spent_by_day(), daily.interrupted_window_start())
    }
}

impl ProviderUsagePoller for CursorUsagePoller {
    fn provider(&self) -> ProviderId {
        ProviderId::Cursor
    }

    fn set_menu_open(&self, open: bool) {
        if self.menu_open.swap(open, Ordering::SeqCst) != open {
            self.polling.wake();
        }
    }

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        lock(&self.state).last_refreshed_at
    }

    fn last_error(&self) -> Option<ProviderError> {
        lock(&self.state).last_error.clone()
    }

    fn headline_used_percent(&self) -> Option<f64> {
        lock(&self.state).snapshot.as_ref().map(|s| s.used_percent)
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        lock(&self.state)
            .snapshot
            .as_ref()
            .and_then(CursorSnapshot::resets_at)
    }

    fn alert_account(&self) -> Option<String> {
        self.session.state().account_email
    }

    fn start(&self) {
        self.polling.start();
    }

    fn stop(&self) {
        self.polling.stop();
    }

    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.polling.refresh_now().await })
    }

    fn clear_snapshot(&self) {
        *lock(&self.state) = State::default();
        (self.clear_event_cache)();
    }
}
