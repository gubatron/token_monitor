//! Poller-side publish rules (spec 02 section 3.4, "Publish").
//!
//! The polling loop itself (gating, backoff, `ProviderRefresh`) lives in
//! ws-core-polling and the app. These pure helpers carry the Cursor-specific
//! decisions so they can be tested without a loop.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use tokenmon_core::Clock;
use tokenmon_core::usage::{Calendar, DailyBudget};

use crate::aggregate::CursorDayHourlyUsage;
use crate::client::CursorFetchResult;
use crate::models::CursorSnapshot;

/// Cycle length used for the daily store when the cycle bounds are unknown.
pub const DEFAULT_CYCLE_DAYS: i64 = 30;

/// What the poller remembers between refreshes for carry-over.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviousPublish {
    /// The previously published snapshot.
    pub snapshot: CursorSnapshot,
    /// The previously published hourly activity.
    pub hourly: CursorDayHourlyUsage,
    /// The weights the previous budget was built from.
    pub estimated_weight_by_day: BTreeMap<NaiveDate, f64>,
}

/// Applies the carry-over rule: when events were unavailable
/// (`cost_stats == None`) and the billing cycle start is unchanged, keep the
/// previous cost stats, daily tokens and (same-day) hourly activity.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use chrono::{TimeZone, Utc};
/// use tokenmon_provider_cursor::{carry_over, parse_summary, CursorCostStats, CursorDayHourlyUsage,
///     CursorFetchResult, PreviousPublish};
///
/// let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
/// let day = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
/// let mut old = parse_summary(b"{}", now).unwrap();
/// old.cost_stats = Some(CursorCostStats { metered_cycle_usd: 7.0, ..Default::default() });
/// let prev = PreviousPublish {
///     snapshot: old,
///     hourly: CursorDayHourlyUsage::empty(day),
///     estimated_weight_by_day: BTreeMap::new(),
/// };
/// let fresh = CursorFetchResult {
///     snapshot: parse_summary(b"{}", now).unwrap(),
///     hourly: CursorDayHourlyUsage::empty(day),
///     estimated_weight_by_day: BTreeMap::new(),
/// };
/// let out = carry_over(fresh, Some(&prev));
/// assert_eq!(out.snapshot.cost_stats.unwrap().metered_cycle_usd, 7.0);
/// ```
pub fn carry_over(new: CursorFetchResult, previous: Option<&PreviousPublish>) -> CursorFetchResult {
    let Some(prev) = previous.filter(|p| {
        new.snapshot.cost_stats.is_none()
            && p.snapshot.billing_cycle_start == new.snapshot.billing_cycle_start
    }) else {
        return new;
    };
    let hourly = if prev.hourly.day_start == new.hourly.day_start {
        prev.hourly.clone()
    } else {
        new.hourly
    };
    let weights = if prev.estimated_weight_by_day.is_empty() {
        new.estimated_weight_by_day
    } else {
        prev.estimated_weight_by_day.clone()
    };
    CursorFetchResult {
        snapshot: CursorSnapshot {
            cost_stats: prev.snapshot.cost_stats,
            daily_tokens: prev.snapshot.daily_tokens.clone(),
            ..new.snapshot
        },
        hourly,
        estimated_weight_by_day: weights,
    }
}

/// `cycleLengthDays`: `daysInBillingCycle` when both ends are known and the
/// end is later, else 30.
pub fn cycle_length_days(
    clock: &dyn Clock,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> i64 {
    match (start, end) {
        (Some(s), Some(e)) if e > s => {
            DailyBudget::days_in_billing_cycle(s, e, &Calendar::local(clock))
        }
        _ => DEFAULT_CYCLE_DAYS,
    }
}

/// Whether the freshly fetched email differs from the stored one, meaning the
/// poller must call `saveAccountEmail` (which may fire `AccountReset`) before
/// publishing.
pub fn account_email_changed(fetched: Option<&str>, stored: Option<&str>) -> bool {
    fetched.is_some_and(|f| Some(f) != stored)
}
