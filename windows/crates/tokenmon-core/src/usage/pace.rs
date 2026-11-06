//! Pace against the pool: period start, headroom and the footer caption.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{Calendar, DailyBudget, DailyBudgetDay};

/// Even-pace headroom vs live period consumption.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaceHeadroom {
    /// One day's share of the pool.
    pub daily_budget: f64,
    /// Allowance accrued through today (elapsed days x daily share).
    pub earned: f64,
    /// Used percentage for the pool (never negative).
    pub period_consumed: f64,
    /// `earned - consumed`: positive is ahead of pace, negative is over pace.
    pub headroom_today: f64,
    /// Reset instant the pool runs to, when known.
    pub resets_at: Option<DateTime<Utc>>,
}

/// Quota window the daily bars pace against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowancePeriod {
    /// Claude / Grok: bars usually cover the full period.
    Weekly,
    /// Cursor / OpenCode: bars are a 7-day slice of a billing month.
    Monthly,
}

/// Error for an unknown [`AllowancePeriod`] string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown allowance period: {0:?}")]
pub struct ParseAllowancePeriodError(pub String);

impl fmt::Display for AllowancePeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        })
    }
}

impl FromStr for AllowancePeriod {
    type Err = ParseAllowancePeriodError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            other => Err(ParseAllowancePeriodError(other.to_owned())),
        }
    }
}

impl DailyBudget {
    /// Weekly providers: start of the first bar that is not a prior-window day.
    #[must_use]
    pub fn weekly_pace_period_start(
        days: &[DailyBudgetDay],
        cal: &Calendar<'_>,
    ) -> Option<DateTime<Utc>> {
        days.iter()
            .find(|d| !d.is_prior_window)
            .map(|d| cal.start_of_day(d.date))
    }

    /// Monthly providers: the subscription month start, else `resets_at` minus
    /// the period length implied by the bars' daily share, else `None`.
    #[must_use]
    pub fn monthly_pace_period_start(
        days: &[DailyBudgetDay],
        known_start: Option<DateTime<Utc>>,
        resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> Option<DateTime<Utc>> {
        if let Some(b) = Self::subscription_month(known_start, resets_at, Some(now), cal) {
            return Some(cal.start_of_day(b.start));
        }
        let daily = days.first().map_or(0.0, |d| d.budget_usd);
        let dip = Self::days_in_period_from_daily_budget(daily, 0);
        let reset_day = cal.start_of_day(resets_at?);
        if dip > 0 {
            cal.add_days(reset_day, -dip)
        } else {
            None
        }
    }

    /// Resolves the pace period start for the allowance kind.
    #[must_use]
    pub fn pace_period_start(
        period: AllowancePeriod,
        days: &[DailyBudgetDay],
        known_start: Option<DateTime<Utc>>,
        resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> Option<DateTime<Utc>> {
        match period {
            AllowancePeriod::Weekly => Self::weekly_pace_period_start(days, cal),
            AllowancePeriod::Monthly => {
                Self::monthly_pace_period_start(days, known_start, resets_at, now, cal)
            }
        }
    }

    /// Days in the period implied by an even daily share of a 100% pool.
    #[must_use]
    pub fn days_in_period_from_daily_budget(daily: f64, fallback: i64) -> i64 {
        if daily > 0.0 {
            ((100.0 / daily).round() as i64).max(1)
        } else {
            fallback.max(0)
        }
    }

    /// Even-pace headroom for the pool, credited in whole calendar days through
    /// today. On the final day, with a known future `resets_at`, the unspent
    /// pool is compared with the even share of the time actually left.
    #[must_use]
    pub fn pace_headroom(
        days: &[DailyBudgetDay],
        period_consumed: f64,
        elapsed_days_in_period: Option<i64>,
        resets_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        cal: &Calendar<'_>,
    ) -> Option<PaceHeadroom> {
        let days: Vec<&DailyBudgetDay> = days.iter().filter(|d| !d.is_prior_window).collect();
        let first = days.first().filter(|d| d.budget_usd > 0.0)?;
        let budget = first.budget_usd;
        let today = cal.start_of_day(now);
        let period_length = Self::days_in_period_from_daily_budget(
            budget,
            i64::try_from(days.len()).unwrap_or(i64::MAX),
        )
        .max(1);
        let elapsed = match elapsed_days_in_period {
            Some(e) => e.clamp(0, period_length),
            None => i64::try_from(
                days.iter()
                    .filter(|d| cal.start_of_day(d.date) <= today)
                    .count(),
            )
            .unwrap_or(i64::MAX),
        };
        let earned = budget * elapsed as f64;
        let consumed = period_consumed.max(0.0);
        let mut headroom = earned - consumed;
        if let Some(reset) = resets_at.filter(|r| *r > now && elapsed >= period_length) {
            let days_until_reset = (reset - now).num_milliseconds() as f64 / 86_400_000.0;
            headroom = (budget * period_length as f64 - consumed) - budget * days_until_reset;
        }
        Some(PaceHeadroom {
            daily_budget: budget,
            earned,
            period_consumed: consumed,
            headroom_today: headroom,
            resets_at,
        })
    }

    /// Footer caption for a pace headroom, shared by every daily chart.
    #[must_use]
    pub fn pace_caption(pace: &PaceHeadroom) -> Option<String> {
        const EPS: f64 = 0.05;
        let left_from_prior = |h: f64| format!("{h:.1}% usage left today from unused prior days");
        if pace.period_consumed <= 0.001 {
            if pace.daily_budget <= 0.0 {
                return None;
            }
            if pace.earned > pace.daily_budget + EPS {
                return Some(left_from_prior(pace.headroom_today));
            }
            return Some(format!("No usage yet · {:.1}% today", pace.daily_budget));
        }
        if pace.headroom_today < 0.0 {
            return Some(format!(
                "Usage {:.1}% over today's allowance",
                -pace.headroom_today
            ));
        }
        if pace.headroom_today > pace.daily_budget + EPS {
            return Some(left_from_prior(pace.headroom_today));
        }
        if pace.headroom_today < EPS {
            return Some("On pace for today".to_owned());
        }
        Some(format!("{:.1}% usage left today", pace.headroom_today))
    }
}
