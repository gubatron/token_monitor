//! Calendar-day budget maths shared by the daily-budget charts.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::{Calendar, DailyTokenUsage, Percent, WeekOffset};

/// Upper bound on bars produced for one period. Server-supplied reset dates can
/// be arbitrary; this keeps a bad date from allocating millions of bars.
pub const MAX_BAR_DAYS: i64 = 3_700;

/// Spend keyed by the start-of-day instant of the local day.
pub type SpentByDay = HashMap<DateTime<Utc>, f64>;

/// One calendar day in a budget chart.
///
/// `spent_usd` / `budget_usd` are nominally USD, but on quota charts they carry
/// percentage points (0..=100); the maths is identical.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyBudgetDay {
    /// Start of the local day.
    pub date: DateTime<Utc>,
    /// Amount spent that day.
    pub spent_usd: f64,
    /// That day's allowance.
    pub budget_usd: f64,
    /// True for a preserved day of a window an early provider reset cut short.
    /// It is history only: pace and elapsed days count the new window alone.
    pub is_prior_window: bool,
    /// Tokens used that day, when the provider can count them.
    pub tokens: Option<DailyTokenUsage>,
}

impl DailyBudgetDay {
    /// A normal (not prior-window) bar without tokens.
    #[must_use]
    pub fn new(date: DateTime<Utc>, spent_usd: f64, budget_usd: f64) -> Self {
        Self {
            date,
            spent_usd,
            budget_usd,
            is_prior_window: false,
            tokens: None,
        }
    }

    /// `clamp(spent / budget * 100)`, or `0` when the budget is not positive.
    #[must_use]
    pub fn percent_of_budget(&self) -> f64 {
        if self.budget_usd > 0.0 {
            Percent::clamp(self.spent_usd / self.budget_usd * 100.0)
        } else {
            0.0
        }
    }

    /// True when spend exceeds a positive budget.
    #[must_use]
    pub fn is_over_budget(&self) -> bool {
        self.spent_usd > self.budget_usd && self.budget_usd > 0.0
    }
}

/// Bounds of a subscription / billing month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthBounds {
    /// Start of the cycle.
    pub start: DateTime<Utc>,
    /// End (reset) of the cycle.
    pub end: DateTime<Utc>,
}

/// Namespace for the budget builders.
#[derive(Debug, Clone, Copy)]
pub struct DailyBudget;

fn shift(cal: &Calendar<'_>, from: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    cal.add_days(from, days).unwrap_or(from)
}

fn bar(
    cal: &Calendar<'_>,
    from: DateTime<Utc>,
    offset: i64,
    spent: &SpentByDay,
    per_day: f64,
) -> DailyBudgetDay {
    let key = cal.start_of_day(shift(cal, from, offset));
    DailyBudgetDay::new(key, spent.get(&key).copied().unwrap_or(0.0), per_day)
}

impl DailyBudget {
    /// Daily allowance from a limit, `0` unless both are positive.
    #[must_use]
    pub fn budget_per_day(limit: f64, days_in_period: i64) -> f64 {
        if limit > 0.0 && days_in_period > 0 {
            limit / days_in_period as f64
        } else {
            0.0
        }
    }

    /// Days in a billing cycle: whole local days between the two days, at least 1.
    #[must_use]
    pub fn days_in_billing_cycle(
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> i64 {
        cal.day_diff(start, end).max(1)
    }

    /// One bar per calendar day of the period.
    ///
    /// When `now` falls on the `period_end` day *before* the reset instant, that
    /// day is included as an extra bar; the daily share stays at the regular day count.
    #[must_use]
    pub fn build_days(
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        limit: f64,
        spent_by_day: &SpentByDay,
        now: Option<DateTime<Utc>>,
        cal: &Calendar<'_>,
    ) -> Vec<DailyBudgetDay> {
        let start = cal.start_of_day(period_start);
        let end_day = cal.start_of_day(period_end);
        let include_end_day =
            now.is_some_and(|n| n >= start && n < period_end && cal.start_of_day(n) == end_day);
        let exclusive_end = if include_end_day {
            shift(cal, end_day, 1)
        } else {
            end_day
        };
        let total = Self::days_in_billing_cycle(start, exclusive_end, cal).min(MAX_BAR_DAYS);
        let period_days = Self::days_in_billing_cycle(start, end_day, cal);
        let per_day = Self::budget_per_day(limit, period_days);
        (0..total)
            .map(|o| bar(cal, start, o, spent_by_day, per_day))
            .collect()
    }

    /// Start of the running weekly pool. Before `resets_at` that is one period
    /// before the reset day; a stale `resets_at` advances by whole periods
    /// (at most 520 steps) so the window still contains today.
    #[must_use]
    pub fn weekly_period_start(
        resets_at: DateTime<Utc>,
        days_in_period: i64,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> DateTime<Utc> {
        let count = days_in_period.max(1);
        let reset_day = cal.start_of_day(resets_at);
        let base = if now >= resets_at {
            let today = cal.start_of_day(now);
            let mut base = reset_day;
            for _ in 0..520 {
                let window_end = shift(cal, base, count - 1);
                if cal.start_of_day(window_end) >= today {
                    break;
                }
                match cal.add_days(base, count) {
                    Some(next) => base = next,
                    None => break,
                }
            }
            base
        } else {
            shift(cal, reset_day, -count)
        };
        cal.start_of_day(base)
    }

    /// Bars for the full weekly window; see the spec (1.4.5) for the anchoring
    /// and prior-window lead-in rules. A non-zero `week_offset` returns exactly
    /// `days_in_period` bars from a full period earlier per step.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn build_weekly_window_days(
        limit: f64,
        days_in_period: i64,
        resets_at: DateTime<Utc>,
        spent_by_day: &SpentByDay,
        window_start: Option<DateTime<Utc>>,
        interrupted_window_start: Option<DateTime<Utc>>,
        week_offset: WeekOffset,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> Vec<DailyBudgetDay> {
        let count = days_in_period.clamp(1, MAX_BAR_DAYS);
        let per_day = Self::budget_per_day(limit, count);
        let mut week_start = Self::weekly_period_start(resets_at, count, now, cal);
        if week_offset != WeekOffset::CURRENT {
            let shifted = shift(cal, week_start, i64::from(week_offset.get()) * count);
            return (0..count)
                .map(|o| bar(cal, shifted, o, spent_by_day, per_day))
                .collect();
        }
        let anchored = match window_start {
            Some(ws) if ws <= now && cal.start_of_day(ws) >= week_start => {
                week_start = cal.start_of_day(ws);
                true
            }
            _ => false,
        };
        let mut days = Vec::new();
        if let (true, Some(interrupted)) = (anchored, interrupted_window_start) {
            let earliest = shift(cal, week_start, -(count - 1));
            let first = cal.start_of_day(interrupted).max(earliest);
            let prior = cal.day_diff(first, week_start).max(0);
            days.extend((0..prior).map(|o| DailyBudgetDay {
                is_prior_window: true,
                ..bar(cal, first, o, spent_by_day, per_day)
            }));
        }
        days.extend((0..count).map(|o| bar(cal, week_start, o, spent_by_day, per_day)));
        days
    }

    /// Monday of the local calendar week containing `now`.
    #[must_use]
    pub fn monday_of_week(now: DateTime<Utc>, cal: &Calendar<'_>) -> DateTime<Utc> {
        let today = cal.start_of_day(now);
        let from_monday = (i64::from(cal.weekday_sunday_first(today)) + 5) % 7;
        shift(cal, today, -from_monday)
    }

    /// Monday-to-Sunday bars for a subscription month. Days outside the billing
    /// period are present with 0 spend (or preserved history, see the spec).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn build_monday_week_days(
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        limit: f64,
        spent_by_day: &SpentByDay,
        prior_window_spent_by_day: &SpentByDay,
        history_by_day: &SpentByDay,
        week_offset: WeekOffset,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> Vec<DailyBudgetDay> {
        let period_days = Self::build_days(
            period_start,
            period_end,
            limit,
            spent_by_day,
            Some(now),
            cal,
        );
        let per_day = period_days.first().map_or_else(
            || {
                Self::budget_per_day(
                    limit,
                    i64::try_from(period_days.len()).unwrap_or(i64::MAX).max(1),
                )
            },
            |d| d.budget_usd,
        );
        let spent_in_period: SpentByDay = period_days
            .iter()
            .map(|d| (cal.start_of_day(d.date), d.spent_usd))
            .collect();
        let base = Self::monday_of_week(now, cal);
        let week_start = shift(cal, base, i64::from(week_offset.get()).saturating_mul(7));
        let period_start_day = cal.start_of_day(period_start);
        (0..7)
            .map(|o| {
                let key = cal.start_of_day(shift(cal, week_start, o));
                if week_offset != WeekOffset::CURRENT && key < period_start_day {
                    let spent = prior_window_spent_by_day
                        .get(&key)
                        .or_else(|| history_by_day.get(&key))
                        .copied()
                        .unwrap_or(0.0);
                    return DailyBudgetDay::new(key, spent, per_day);
                }
                if let Some(prior) = prior_window_spent_by_day
                    .get(&key)
                    .filter(|_| key < period_start_day)
                {
                    return DailyBudgetDay {
                        is_prior_window: true,
                        ..DailyBudgetDay::new(key, *prior, per_day)
                    };
                }
                DailyBudgetDay::new(
                    key,
                    spent_in_period.get(&key).copied().unwrap_or(0.0),
                    per_day,
                )
            })
            .collect()
    }

    /// Inclusive local days from `period_start` through today; `0` if today is before the start.
    #[must_use]
    pub fn elapsed_days_through_today(
        period_start: DateTime<Utc>,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> i64 {
        let start = cal.start_of_day(period_start);
        let today = cal.start_of_day(now);
        if today < start {
            0
        } else {
            cal.day_diff(start, today) + 1
        }
    }

    /// Subscription / billing month bounds anchored to the billing cycle.
    ///
    /// With one side known the other is inferred a calendar month away. With
    /// `now`, a stale cycle advances month by month (at most 520 steps), each
    /// boundary computed from the anchor so a Jan 31 anchor gives Feb 28 then Mar 31.
    #[must_use]
    pub fn subscription_month(
        known_start: Option<DateTime<Utc>>,
        resets_at: Option<DateTime<Utc>>,
        now: Option<DateTime<Utc>>,
        cal: &Calendar<'_>,
    ) -> Option<MonthBounds> {
        let (mut start, mut end, anchor, mut end_offset) = match (known_start, resets_at) {
            (Some(s), Some(r)) if r > s => (s, r, r, 0_i32),
            (Some(s), _) => {
                let e = cal.add_months(s, 1).filter(|e| *e > s)?;
                (s, e, s, 1)
            }
            (None, Some(r)) => {
                let s = cal.add_months(r, -1).filter(|s| r > *s)?;
                (s, r, r, 0)
            }
            (None, None) => return None,
        };
        if let Some(now) = now {
            for _ in 0..520 {
                if end > now {
                    break;
                }
                let Some(advanced) = cal
                    .add_months(anchor, end_offset.saturating_add(1))
                    .filter(|a| *a > end)
                else {
                    break;
                };
                start = end;
                end = advanced;
                end_offset += 1;
            }
        }
        Some(MonthBounds { start, end })
    }

    /// Monday-to-Sunday bars for a subscription month; `None` when neither the
    /// cycle start nor the reset is known. Returns the bars and the start of the cycle's first day.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn build_subscription_month_last_7_days(
        limit: f64,
        spent_by_day: &SpentByDay,
        known_start: Option<DateTime<Utc>>,
        resets_at: Option<DateTime<Utc>>,
        history_by_day: &SpentByDay,
        week_offset: WeekOffset,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> Option<(Vec<DailyBudgetDay>, DateTime<Utc>)> {
        let bounds = Self::subscription_month(known_start, resets_at, Some(now), cal)?;
        let days = Self::build_monday_week_days(
            bounds.start,
            bounds.end,
            limit,
            spent_by_day,
            &SpentByDay::new(),
            history_by_day,
            week_offset,
            now,
            cal,
        );
        Some((days, cal.start_of_day(bounds.start)))
    }
}
