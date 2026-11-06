//! Daily-budget blending for the monthly Cursor bars (spec 02 section 3.4,
//! `buildDailyBudgetDays` steps 1 to 6).
//!
//! The Monday-to-Sunday bar construction (`buildMondayWeekDays`) belongs to
//! ws-core-usage; this module produces the per-day values it consumes.
//! Inputs may be extreme (fuzzed): every output value is finite and
//! non-negative.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use tokenmon_core::Clock;
use tokenmon_core::usage::{Calendar, DailyBudget};

/// Inputs of [`blend_daily_budget`].
#[derive(Debug, Clone, Copy)]
pub struct BlendInputs<'a> {
    /// Per-day percent points measured by the daily delta store.
    pub observed_by_day: &'a BTreeMap<NaiveDate, f64>,
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
    /// Reference time.
    pub now: DateTime<Utc>,
}

/// Per-day values for the current billing month.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendedBudget {
    /// Billing-month bounds.
    pub start: DateTime<Utc>,
    /// Exclusive end of the billing month.
    pub end: DateTime<Utc>,
    /// Percent points per day of this cycle (observed, rescaled or back-filled).
    pub blended: BTreeMap<NaiveDate, f64>,
    /// Observed days of the interrupted earlier window (shown dimmed).
    pub prior: BTreeMap<NaiveDate, f64>,
}

fn sane(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// Blends measured and estimated per-day usage; `None` when no billing month
/// can be derived (a calendar month is never invented).
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use chrono::{NaiveDate, TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_provider_cursor::{blend_daily_budget, BlendInputs};
///
/// let day = |d| NaiveDate::from_ymd_opt(2026, 8, d).unwrap();
/// let observed = BTreeMap::from([(day(23), 1.0), (day(25), 6.0), (day(26), 4.0)]);
/// let estimated = BTreeMap::new();
/// let clock = ManualClock::from_unix(0);
/// let out = blend_daily_budget(&clock, BlendInputs {
///     observed_by_day: &observed,
///     interrupted_window_start: None,
///     estimated_weight_by_day: &estimated,
///     used_percent: 5.0,
///     cycle_start: Some(Utc.with_ymd_and_hms(2026, 8, 22, 17, 0, 0).unwrap()),
///     cycle_end: Some(Utc.with_ymd_and_hms(2026, 9, 22, 17, 0, 0).unwrap()),
///     now: Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap(),
/// }).unwrap();
/// assert_eq!(out.blended[&day(25)], 3.0);
/// assert_eq!(out.blended[&day(26)], 2.0);
/// ```
pub fn blend_daily_budget(clock: &dyn Clock, input: BlendInputs<'_>) -> Option<BlendedBudget> {
    let cal = Calendar::local(clock);
    let bounds =
        DailyBudget::subscription_month(input.cycle_start, input.cycle_end, Some(input.now), &cal)?;
    let (start, end) = (bounds.start, bounds.end);
    let cycle_start_day = cal.date_of(start);
    let in_cycle = |day: &NaiveDate| {
        *day >= cycle_start_day
            && cal
                .start_of_date(*day)
                .is_some_and(|midnight| midnight < end)
    };
    let observed: BTreeMap<NaiveDate, f64> = input
        .observed_by_day
        .iter()
        .filter(|(d, _)| in_cycle(d))
        .map(|(d, v)| (*d, sane(*v)))
        .collect();
    let estimated: BTreeMap<NaiveDate, f64> = input
        .estimated_weight_by_day
        .iter()
        .filter(|(d, _)| in_cycle(d))
        .map(|(d, v)| (*d, sane(*v)))
        .collect();
    // The earliest observed day was only partially tracked; drop it.
    let effective: BTreeMap<NaiveDate, f64> =
        observed.iter().skip(1).map(|(d, v)| (*d, *v)).collect();
    let blended = fill(&effective, &estimated, sane(input.used_percent));
    let prior = input
        .interrupted_window_start
        .map(|w| cal.date_of(w))
        .map(|from| {
            input
                .observed_by_day
                .iter()
                .filter(|(d, _)| **d >= from && **d < cycle_start_day)
                .map(|(d, v)| (*d, sane(*v)))
                .collect()
        })
        .unwrap_or_default();
    Some(BlendedBudget {
        start,
        end,
        blended,
        prior,
    })
}

fn fill(
    effective: &BTreeMap<NaiveDate, f64>,
    estimated: &BTreeMap<NaiveDate, f64>,
    used: f64,
) -> BTreeMap<NaiveDate, f64> {
    let tracked: f64 = effective.values().sum();
    let mut blended = effective.clone();
    if tracked > used + 0.001 && tracked > 0.0 {
        let scale = used / tracked;
        for v in blended.values_mut() {
            *v *= scale;
        }
        return blended;
    }
    let untracked = (used - tracked).max(0.0);
    let unobserved: Vec<(NaiveDate, f64)> = estimated
        .iter()
        .filter(|(d, _)| !effective.contains_key(d))
        .map(|(d, w)| (*d, *w))
        .collect();
    let weight_sum: f64 = unobserved.iter().map(|(_, w)| w).sum();
    if untracked > 0.001 && weight_sum > 0.0 {
        for (day, weight) in &unobserved {
            *blended.entry(*day).or_insert(0.0) += weight / weight_sum * untracked;
        }
    } else if untracked > 0.001 && !unobserved.is_empty() {
        let share = untracked / unobserved.len() as f64;
        for (day, _) in &unobserved {
            *blended.entry(*day).or_insert(0.0) += share;
        }
    }
    blended
}
