//! Daily-chart input for the provider panels: converts each poller's
//! [`DailyBudgetDay`] bars (and Grok's [`DailyUsageWeek`]) into the UI's
//! [`DailyBudgetInput`] (spec 04 section 3.3).
//!
//! Pure and portable: the poller supplies the bars, this module decides pace,
//! navigation state and the over-budget flag.

use chrono::{DateTime, Utc};
use tokenmon_core::ProviderId;
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay, DailyTokenUsage};
use tokenmon_provider_grok::models::{DAILY_CAP_PERCENT, DailyUsageWeek};
use tokenmon_ui::panels::common::{
    AllowanceNoun, DailyBudgetInput, DayInput, FooterKind, TokensInput,
};

/// Smallest fill fraction that draws a bar for the generic chart.
const GENERIC_FILL_THRESHOLD: f64 = 0.005;

/// Spent beyond the allowance by more than this counts as over budget.
const OVER_BUDGET_EPSILON: f64 = 1e-9;

/// What the chart for one provider needs besides the bars.
#[derive(Debug, Clone, Copy)]
pub struct ChartContext<'a> {
    /// Provider the chart belongs to (decides the accent).
    pub provider: ProviderId,
    /// Weekly or monthly wording.
    pub noun: AllowanceNoun,
    /// Used percent of the pool, for the pace footer.
    pub period_used_percent: f64,
    /// When the pool resets, if known.
    pub resets_at: Option<DateTime<Utc>>,
    /// Browsed period: `0` current, negative earlier.
    pub week_offset: i32,
    /// Current instant.
    pub now: DateTime<Utc>,
    /// Local calendar.
    pub calendar: &'a Calendar<'a>,
}

/// The allowance wording of a provider's daily chart: Cursor and OpenCode
/// track a monthly plan, everyone else a weekly pool.
pub fn noun_for(provider: ProviderId) -> AllowanceNoun {
    match provider {
        ProviderId::Cursor | ProviderId::OpenCode => AllowanceNoun::Monthly,
        _ => AllowanceNoun::Weekly,
    }
}

fn tokens(t: &DailyTokenUsage) -> TokensInput {
    let count = |n: i64| u64::try_from(n).unwrap_or(0);
    TokensInput {
        total: count(t.total_tokens()),
        input: count(t.input_tokens),
        output: count(t.output_tokens),
        cache_read: count(t.cache_read_tokens),
        cache_write: count(t.cache_write_tokens),
        cost_usd: t.cost_usd.unwrap_or(0.0),
        has_unpriced: t.has_unpriced,
        models: t
            .models
            .iter()
            .map(|m| (m.name.clone(), count(m.tokens)))
            .collect(),
    }
}

fn day_input(day: &DailyBudgetDay, calendar: &Calendar<'_>) -> DayInput {
    DayInput {
        date: calendar.date_of(day.date),
        spent: day.spent_usd,
        budget: day.budget_usd,
        is_prior_window: day.is_prior_window,
        is_over_budget: day.spent_usd > day.budget_usd + OVER_BUDGET_EPSILON,
        tokens: day.tokens.as_ref().map(tokens),
    }
}

/// The chart for a provider's bars, or `None` when there are none.
///
/// Pace is shown only for the current period; browsing earlier periods hides
/// the live footer and enables the forward arrow.
pub fn daily_input(days: &[DailyBudgetDay], ctx: &ChartContext<'_>) -> Option<DailyBudgetInput> {
    if days.is_empty() {
        return None;
    }
    let current = ctx.week_offset >= 0;
    let pace = current
        .then(|| {
            DailyBudget::pace_headroom(
                days,
                ctx.period_used_percent,
                None,
                ctx.resets_at,
                ctx.now,
                ctx.calendar,
            )
        })
        .flatten();
    Some(DailyBudgetInput {
        days: days.iter().map(|d| day_input(d, ctx.calendar)).collect(),
        accent: ctx.provider.panel_accent(),
        noun: ctx.noun,
        pace,
        footer_kind: FooterKind::Generic,
        show_live_footer: current,
        has_navigation: true,
        can_go_next: !current,
        fill_threshold: GENERIC_FILL_THRESHOLD,
    })
}

/// The chart for Grok's per-product week, or `None` when it has no days.
pub fn grok_week_input(week: &DailyUsageWeek, ctx: &ChartContext<'_>) -> Option<DailyBudgetInput> {
    if week.days.is_empty() {
        return None;
    }
    let current = ctx.week_offset >= 0;
    let bars: Vec<DailyBudgetDay> = week
        .days
        .iter()
        .map(|d| DailyBudgetDay::new(d.day_start, d.total_percent(), DAILY_CAP_PERCENT))
        .collect();
    let pace = current
        .then(|| {
            DailyBudget::pace_headroom(
                &bars,
                ctx.period_used_percent,
                None,
                ctx.resets_at,
                ctx.now,
                ctx.calendar,
            )
        })
        .flatten();
    Some(DailyBudgetInput {
        days: bars.iter().map(|d| day_input(d, ctx.calendar)).collect(),
        accent: ProviderId::Grok.panel_accent(),
        noun: AllowanceNoun::Weekly,
        pace,
        footer_kind: FooterKind::GrokWeek {
            show_estimate_note: week.is_estimated || !week.has_daily_data,
        },
        show_live_footer: current,
        has_navigation: true,
        can_go_next: !current,
        fill_threshold: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ctx<'a>(cal: &'a Calendar<'a>, offset: i32) -> ChartContext<'a> {
        ChartContext {
            provider: ProviderId::Claude,
            noun: AllowanceNoun::Weekly,
            period_used_percent: 30.0,
            resets_at: None,
            week_offset: offset,
            now: Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap(),
            calendar: cal,
        }
    }

    fn bars() -> Vec<DailyBudgetDay> {
        (0..7)
            .map(|i| {
                DailyBudgetDay::new(
                    Utc.with_ymd_and_hms(2026, 7, 6 + i, 0, 0, 0).unwrap(),
                    if i == 0 { 40.0 } else { 1.0 },
                    100.0 / 7.0,
                )
            })
            .collect()
    }

    /// No bars means no chart card.
    #[test]
    fn test_empty_bars_give_no_chart() {
        let cal = Calendar::utc();
        assert!(daily_input(&[], &ctx(&cal, 0)).is_none());
    }

    /// The current period shows the live footer and cannot go forward; the
    /// over-budget flag follows spent versus allowance.
    #[test]
    fn test_current_period_navigation_and_over_budget() {
        let cal = Calendar::utc();
        let input = daily_input(&bars(), &ctx(&cal, 0)).unwrap();
        assert!(input.show_live_footer && !input.can_go_next && input.has_navigation);
        assert!(input.days[0].is_over_budget);
        assert!(!input.days[1].is_over_budget);
        assert!(input.pace.is_some());
    }

    /// An earlier period drops pace and the live footer and allows forward.
    #[test]
    fn test_earlier_period_hides_live_footer() {
        let cal = Calendar::utc();
        let input = daily_input(&bars(), &ctx(&cal, -1)).unwrap();
        assert!(!input.show_live_footer && input.can_go_next && input.pace.is_none());
    }

    /// Cursor and OpenCode say monthly; the rest weekly.
    #[test]
    fn test_noun_per_provider() {
        assert_eq!(noun_for(ProviderId::Cursor), AllowanceNoun::Monthly);
        assert_eq!(noun_for(ProviderId::OpenCode), AllowanceNoun::Monthly);
        assert_eq!(noun_for(ProviderId::Grok), AllowanceNoun::Weekly);
        assert_eq!(noun_for(ProviderId::Claude), AllowanceNoun::Weekly);
    }

    /// Token counts carry over with negatives floored at zero.
    #[test]
    fn test_tokens_carry_over() {
        let cal = Calendar::utc();
        let mut days = bars();
        days[0].tokens = Some(DailyTokenUsage {
            input_tokens: 10,
            output_tokens: -5,
            ..DailyTokenUsage::default()
        });
        let input = daily_input(&days, &ctx(&cal, 0)).unwrap();
        let t = input.days[0].tokens.as_ref().unwrap();
        assert_eq!((t.input, t.output), (10, 0));
    }
}
