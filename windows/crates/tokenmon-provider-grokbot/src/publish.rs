//! Poller-side publish state (spec 02 section 2.5).
//!
//! The polling loop (gating, backoff) lives in ws-core-polling and the app.
//! [`GrokbotPublisher`] holds what the Grokbot poller keeps between refreshes:
//! the daily and hourly delta stores, the last snapshot and the bars. It is a
//! plain struct; the app wraps it in a mutex.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokenmon_core::Clock;
use tokenmon_core::storage::{
    DailyQuotaDeltaStore, HourlyDeltaActivityStore, QuotaWindow, QuotaWindowTransition, TextStore,
};
use tokenmon_core::usage::budget::SpentByDay;
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay, WeekOffset};

use crate::models::{DEFAULT_PERIOD_DAYS, GrokbotSnapshot};

/// Storage key of the daily delta store (file `activity_grokbot_daily_usage.dat`).
pub const DAILY_STORE_KEY: &str = "grokbot_daily_usage";
/// Storage key of the hourly delta store (file `activity_grokbot_hourly_today.dat`).
pub const HOURLY_STORE_KEY: &str = "grokbot_hourly_today";
/// Bars are in pool percent, so the limit is 100.
pub const POOL_LIMIT: f64 = 100.0;

/// Everything the Grokbot poller remembers between refreshes.
pub struct GrokbotPublisher {
    clock: Arc<dyn Clock>,
    daily: DailyQuotaDeltaStore,
    hourly: HourlyDeltaActivityStore,
    snapshot: Option<GrokbotSnapshot>,
    weekly_resets_at: Option<DateTime<Utc>>,
    budget_reference_now: Option<DateTime<Utc>>,
}

impl GrokbotPublisher {
    /// Opens the two history stores in `store` (a per-user text store).
    pub fn new(store: &Arc<dyn TextStore>, clock: Arc<dyn Clock>) -> Self {
        Self {
            daily: DailyQuotaDeltaStore::new(
                Arc::clone(store),
                DAILY_STORE_KEY,
                Arc::clone(&clock),
            ),
            hourly: HourlyDeltaActivityStore::new(
                Arc::clone(store),
                HOURLY_STORE_KEY,
                Arc::clone(&clock),
            ),
            clock,
            snapshot: None,
            weekly_resets_at: None,
            budget_reference_now: None,
        }
    }

    /// The last published snapshot.
    pub fn snapshot(&self) -> Option<&GrokbotSnapshot> {
        self.snapshot.as_ref()
    }

    /// The daily history store (read access for tests and the panel).
    pub fn daily(&self) -> &DailyQuotaDeltaStore {
        &self.daily
    }

    /// The hourly history store.
    pub fn hourly(&self) -> &HourlyDeltaActivityStore {
        &self.hourly
    }

    /// Publishes a fresh snapshot: remembers the reset, records the daily and
    /// hourly deltas and handles the window transition.
    pub fn publish(&mut self, snapshot: GrokbotSnapshot) -> QuotaWindowTransition {
        let cal = Calendar::local(self.clock.as_ref());
        let days = snapshot.days_in_period(&cal);
        let at = snapshot.fetched_at;
        if snapshot.resets_at.is_some() {
            self.weekly_resets_at = snapshot.resets_at;
        }
        let window = QuotaWindow {
            start: snapshot.period_start,
            resets_at: snapshot.resets_at,
        };
        let transition = self
            .daily
            .record(snapshot.used_percent, at, Some(window), days);
        match transition {
            QuotaWindowTransition::Rollover => self.hourly.begin_new_window(false),
            QuotaWindowTransition::EarlyReset => self.hourly.begin_new_window(true),
            QuotaWindowTransition::None => {}
        }
        self.hourly.record(snapshot.used_percent, at);
        self.budget_reference_now = Some(at);
        self.snapshot = Some(snapshot);
        transition
    }

    /// Sign-out: drops the displayed snapshot but keeps the history stores.
    pub fn clear_snapshot(&mut self) {
        self.snapshot = None;
        self.weekly_resets_at = None;
        self.budget_reference_now = None;
    }

    /// Account reset: also clears the history stores.
    pub fn account_reset(&mut self) {
        self.clear_snapshot();
        self.daily.clear();
        self.hourly.clear(self.clock.now());
    }

    /// `dailyBudgetDays(weekOffset:)`: bars rebuilt from the live store, the
    /// snapshot's reset (or the last remembered one) and its period length.
    pub fn daily_budget_days(&self, week_offset: WeekOffset) -> Vec<DailyBudgetDay> {
        let cal = Calendar::local(self.clock.as_ref());
        let resets_at = self
            .snapshot
            .as_ref()
            .and_then(|s| s.resets_at)
            .or(self.weekly_resets_at);
        let days_in_period = self
            .snapshot
            .as_ref()
            .map_or(DEFAULT_PERIOD_DAYS, |s| s.days_in_period(&cal));
        let spent: SpentByDay = self.daily.spent_by_day().into_iter().collect();
        build_daily_budget_days(
            &spent,
            resets_at,
            days_in_period,
            self.daily.window_start(),
            self.daily.interrupted_window_start(),
            week_offset,
            self.budget_reference_now
                .unwrap_or_else(|| self.clock.now()),
            &cal,
        )
    }
}

/// `buildDailyBudgetDays`: empty without a reset, otherwise the weekly window
/// bars in pool percent (`budget/day = 100 / days_in_period`).
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::usage::{Calendar, WeekOffset};
/// use tokenmon_provider_grokbot::build_daily_budget_days;
/// let now = Utc.with_ymd_and_hms(2026, 8, 25, 15, 0, 0).unwrap();
/// let bars = build_daily_budget_days(
///     &HashMap::new(), None, 7, None, None, WeekOffset::CURRENT, now, &Calendar::utc());
/// assert!(bars.is_empty());
/// ```
#[allow(clippy::too_many_arguments)]
pub fn build_daily_budget_days(
    spent_by_day: &SpentByDay,
    resets_at: Option<DateTime<Utc>>,
    days_in_period: i64,
    window_start: Option<DateTime<Utc>>,
    interrupted_window_start: Option<DateTime<Utc>>,
    week_offset: WeekOffset,
    now: DateTime<Utc>,
    cal: &Calendar<'_>,
) -> Vec<DailyBudgetDay> {
    let Some(resets_at) = resets_at else {
        return Vec::new();
    };
    DailyBudget::build_weekly_window_days(
        POOL_LIMIT,
        days_in_period,
        resets_at,
        spent_by_day,
        window_start,
        interrupted_window_start,
        week_offset,
        now,
        cal,
    )
}
