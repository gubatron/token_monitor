//! Regression tests for #54 and #60: pace over the billing month, and no
//! pace without a used percent.
#![allow(clippy::unwrap_used)]

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_app::daily::{ChartContext, daily_input};
use tokenmon_core::ProviderId;
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay};
use tokenmon_ui::panels::common::AllowanceNoun;

fn utc(m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, m, d, 0, 0, 0).unwrap()
}

/// Mon Nov 2 .. Sun Nov 8 of a 31-day cycle (3.226%/day).
fn week_bars() -> Vec<DailyBudgetDay> {
    (2..=8)
        .map(|d| DailyBudgetDay::new(utc(11, d), 0.0, 100.0 / 31.0))
        .collect()
}

fn cursor_ctx<'a>(cal: &'a Calendar<'a>, used: Option<f64>) -> ChartContext<'a> {
    ChartContext {
        provider: ProviderId::Cursor,
        noun: AllowanceNoun::Monthly,
        period_used_percent: used,
        period_start: Some(utc(10, 20)),
        resets_at: Some(utc(11, 20)),
        week_offset: 0,
        now: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
        calendar: cal,
    }
}

/// #54: Cursor cycle Oct 20 - Nov 20, Fri Nov 6, 40% used: 18 elapsed days
/// earn 58.06%, so 18.1% is left (not "over today's allowance" from counting
/// only the five bars of this week).
#[test]
fn test_monthly_pace_counts_days_elapsed_in_the_billing_month() {
    let cal = Calendar::utc();
    let input = daily_input(&week_bars(), &cursor_ctx(&cal, Some(40.0))).unwrap();
    let pace = input.pace.unwrap();
    assert!((pace.earned - 18.0 * 100.0 / 31.0).abs() < 1e-6);
    assert_eq!(
        DailyBudget::pace_caption(&pace).unwrap(),
        "18.1% usage left today from unused prior days"
    );
}

/// With no known start, the month is derived from `resets_at`; with neither
/// there is no monthly pace.
#[test]
fn test_monthly_pace_falls_back_to_resets_at_then_to_none() {
    let cal = Calendar::utc();
    let mut ctx = cursor_ctx(&cal, Some(40.0));
    ctx.period_start = None;
    let pace = daily_input(&week_bars(), &ctx).unwrap().pace.unwrap();
    assert!((pace.earned - 18.0 * 100.0 / 31.0).abs() < 1e-6);

    ctx.resets_at = None;
    assert!(daily_input(&week_bars(), &ctx).unwrap().pace.is_none());
}

/// #60: no used percent (Claude without a seven-day window) means no pace,
/// not a pace against 0%.
#[test]
fn test_missing_used_percent_shows_no_pace() {
    let cal = Calendar::utc();
    assert!(
        daily_input(&week_bars(), &cursor_ctx(&cal, None))
            .unwrap()
            .pace
            .is_none()
    );
    let mut ctx = cursor_ctx(&cal, None);
    ctx.noun = AllowanceNoun::Weekly;
    assert!(daily_input(&week_bars(), &ctx).unwrap().pace.is_none());
}

/// Weekly pools still earn from the first non-prior bar.
#[test]
fn test_weekly_pace_still_counts_visible_bars() {
    let cal = Calendar::utc();
    let mut ctx = cursor_ctx(&cal, Some(10.0));
    ctx.noun = AllowanceNoun::Weekly;
    ctx.period_start = None;
    let pace = daily_input(&week_bars(), &ctx).unwrap().pace.unwrap();
    assert!((pace.earned - 5.0 * 100.0 / 31.0).abs() < 1e-6);
}
