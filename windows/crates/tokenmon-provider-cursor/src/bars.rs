//! `buildDailyBudgetDays`: the Monday-to-Sunday bars of the Cursor panel
//! (spec 02 section 3.4), composed from [`blend_daily_budget`] and core's
//! [`DailyBudget::build_monday_week_days`].

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use tokenmon_core::Clock;
use tokenmon_core::usage::budget::SpentByDay;
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay, WeekOffset};

use crate::budget::{BlendInputs, blend_daily_budget};

/// Daily allowance base of the Cursor bars: percent points of the 100 % pool.
pub const POOL_LIMIT: f64 = 100.0;

/// Inputs of [`build_daily_budget_days`].
#[derive(Debug, Clone, Copy)]
pub struct DailyBudgetInputs<'a> {
    /// Per-day percent points from the daily delta store, keyed by local
    /// start of day (`DailyQuotaDeltaStore::spent_by_day`).
    pub observed_by_day: &'a SpentByDay,
    /// Start of the previous window when the cycle was cut short (early reset).
    pub interrupted_window_start: Option<DateTime<Utc>>,
    /// Token weight per day (usage estimate for untracked days).
    pub estimated_weight_by_day: &'a BTreeMap<NaiveDate, f64>,
    /// Cycle used percent (0..=100).
    pub used_percent: f64,
    /// Known cycle start.
    pub cycle_start: Option<DateTime<Utc>>,
    /// Known cycle end.
    pub cycle_end: Option<DateTime<Utc>>,
    /// Monday-week offset: `0` is the week containing `now`, `-1` the previous.
    pub week_offset: WeekOffset,
    /// Reference time.
    pub now: DateTime<Utc>,
}

/// `buildDailyBudgetDays`: the seven Monday-to-Sunday bars for the current
/// billing cycle, in pool percent (`budget/day = 100 / cycle days`).
///
/// Measured days use the real day-over-day growth of the pool percent; the
/// rest are back-filled by [`blend_daily_budget`]. Days before the cycle start
/// are hidden, except that after an early reset the interrupted window's days
/// show as dimmed history and a previous-week offset reads preserved history.
/// `None` when no billing month can be derived (a calendar month is never
/// invented).
///
/// # Examples
///
/// ```
/// use std::collections::{BTreeMap, HashMap};
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::usage::WeekOffset;
/// use tokenmon_core::ManualClock;
/// use tokenmon_provider_cursor::{build_daily_budget_days, DailyBudgetInputs};
///
/// let clock = ManualClock::from_unix(0);
/// let none = build_daily_budget_days(&clock, DailyBudgetInputs {
///     observed_by_day: &HashMap::new(),
///     interrupted_window_start: None,
///     estimated_weight_by_day: &BTreeMap::new(),
///     used_percent: 2.0,
///     cycle_start: None,
///     cycle_end: None,
///     week_offset: WeekOffset::CURRENT,
///     now: Utc.with_ymd_and_hms(2026, 8, 25, 12, 0, 0).unwrap(),
/// });
/// assert!(none.is_none());
/// ```
pub fn build_daily_budget_days(
    clock: &dyn Clock,
    input: DailyBudgetInputs<'_>,
) -> Option<Vec<DailyBudgetDay>> {
    let cal = Calendar::local(clock);
    let observed = by_date(&cal, input.observed_by_day);
    let blended = blend_daily_budget(
        clock,
        BlendInputs {
            observed_by_day: &observed,
            interrupted_window_start: input.interrupted_window_start,
            estimated_weight_by_day: input.estimated_weight_by_day,
            used_percent: input.used_percent,
            cycle_start: input.cycle_start,
            cycle_end: input.cycle_end,
            now: input.now,
        },
    )?;
    Some(DailyBudget::build_monday_week_days(
        blended.start,
        blended.end,
        POOL_LIMIT,
        &by_instant(&cal, &blended.blended),
        &by_instant(&cal, &blended.prior),
        input.observed_by_day,
        input.week_offset,
        input.now,
        &cal,
    ))
}

fn by_date(cal: &Calendar<'_>, spent: &SpentByDay) -> BTreeMap<NaiveDate, f64> {
    spent.iter().fold(BTreeMap::new(), |mut acc, (day, v)| {
        *acc.entry(cal.date_of(*day)).or_insert(0.0) += v;
        acc
    })
}

fn by_instant(cal: &Calendar<'_>, days: &BTreeMap<NaiveDate, f64>) -> SpentByDay {
    days.iter()
        .filter_map(|(d, v)| Some((cal.start_of_date(*d)?, *v)))
        .collect()
}
