//! Weekly daily-budget bars and the Claude Code tokens shown on hover.
//!
//! The bars are percent-of-weekly-pool: limit 100 over 7 days. The poller
//! (ws-app wiring) feeds the live `DailyQuotaDeltaStore` values in; this
//! module is the pure part of `ClaudeUsagePoller`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use tokenmon_core::usage::{
    Calendar, DailyBudget, DailyBudgetDay, DailyTokenUsage, DayKey, TokenCounts, WeekOffset,
    attaching_tokens,
};

use crate::pricing::{cost, display_name};
use crate::tokens::{DailyTokens, ModelTokens};
use crate::usage::project_weekly_reset;

/// Spent percentage points keyed by the start-of-day instant.
pub type SpentByDay = std::collections::HashMap<DateTime<Utc>, f64>;

/// The weekly pool is 100 percentage points over 7 days.
pub const WEEKLY_LIMIT_PERCENT: f64 = 100.0;
/// Bars per weekly window.
pub const DAYS_IN_PERIOD: i64 = 7;

/// The seven weekly bars (plus any prior-window lead-in). `resets_at` of
/// `None` yields no bars: a rolling window is never substituted.
#[allow(clippy::too_many_arguments)]
pub fn build_daily_budget_days(
    spent_by_day: &SpentByDay,
    resets_at: Option<DateTime<Utc>>,
    window_start: Option<DateTime<Utc>>,
    interrupted_window_start: Option<DateTime<Utc>>,
    week_offset: WeekOffset,
    now: DateTime<Utc>,
    calendar: &Calendar<'_>,
) -> Vec<DailyBudgetDay> {
    let Some(resets_at) = resets_at else {
        return Vec::new();
    };
    DailyBudget::build_weekly_window_days(
        WEEKLY_LIMIT_PERCENT,
        DAYS_IN_PERIOD,
        resets_at,
        spent_by_day,
        window_start,
        interrupted_window_start,
        week_offset,
        now,
        calendar,
    )
}

/// Reset time for the weekly bars: the live one, else the last known one
/// projected forward by whole weeks.
pub fn weekly_resets_at(
    live: Option<DateTime<Utc>>,
    remembered: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    live.or_else(|| project_weekly_reset(remembered, now))
}

/// One local day's tokens as the shared hover-label model: per-model usage
/// merged by display name, cache writes summed over lifetimes, API cost
/// accumulated for priced models.
pub fn daily_token_usage(models: &BTreeMap<String, ModelTokens>) -> DailyTokenUsage {
    let mut day = DailyTokenUsage::default();
    for (id, t) in models {
        let counts = TokenCounts {
            input: t.input_tokens,
            output: t.output_tokens,
            cache_read: t.cache_read_tokens,
            cache_write: t
                .cache_write_5m_tokens
                .saturating_add(t.cache_write_1h_tokens),
        };
        day.add(&display_name(id), counts, cost(t, id));
    }
    day
}

/// Attaches each bar's local-day tokens. An empty `local_daily_tokens`
/// returns the bars unchanged; days without transcripts keep `tokens: None`.
pub fn attach_tokens(
    days: Vec<DailyBudgetDay>,
    local_daily_tokens: &DailyTokens,
    calendar: &Calendar<'_>,
) -> Vec<DailyBudgetDay> {
    if local_daily_tokens.is_empty() {
        return days;
    }
    attaching_tokens(days, calendar, |key: &DayKey| {
        local_daily_tokens.get(key.as_str()).map(daily_token_usage)
    })
}
