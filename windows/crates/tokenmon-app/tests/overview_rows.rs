//! Overview rows of the Swift `OpenCodeStatsTests` that exercise the shared
//! hourly model and the chart's bar maths (they live in the app crate because
//! they cross core and UI).

use chrono::NaiveDate;
use tokenmon_core::overview::{OverviewDaySeries, OverviewSeries, ProviderDayHourlyUsage};
use tokenmon_ui::panels::overview::bar_height;

const TRACK: f64 = 100.0;

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("date")
}

fn zeros() -> Vec<f64> {
    vec![0.0; 24]
}

/// Hourly activity is the sum of quota-percent deltas, never normalised per
/// day: 2 + 1 gives 3, 0.5 stays 0.5, and Grok's share is two thirds.
#[test]
fn test_provider_hourly_usage_uses_quota_deltas_without_daily_normalization() {
    let (mut grok, mut go) = (zeros(), zeros());
    grok[10] = 2.0;
    go[10] = 1.0;
    grok[11] = 0.5;
    let series = OverviewDaySeries::default()
        .with_weights(OverviewSeries::Grok, grok)
        .with_weights(OverviewSeries::OpenCodeGo, go);
    let usage = ProviderDayHourlyUsage::build(day(), &series).expect("has activity");
    assert!((usage.hours[10].activity - 3.0).abs() < 1e-3);
    assert!((usage.hours[11].activity - 0.5).abs() < 1e-3);
    let share = usage.hours[10].shares[OverviewSeries::Grok.index()];
    assert!((share - 2.0 / 3.0 * 100.0).abs() < 1e-3);
}

/// Grokbot joins the stack: 4 of 5 points is an 80 % share.
#[test]
fn test_provider_hourly_usage_includes_grokbot_share() {
    let (mut grok, mut bot) = (zeros(), zeros());
    grok[10] = 1.0;
    bot[10] = 4.0;
    let series = OverviewDaySeries::default()
        .with_weights(OverviewSeries::Grok, grok)
        .with_weights(OverviewSeries::Grokbot, bot);
    let usage = ProviderDayHourlyUsage::build(day(), &series).expect("has activity");
    let hour = &usage.hours[10];
    assert!((hour.activity - 5.0).abs() < 1e-3);
    assert!((hour.shares[OverviewSeries::Grokbot.index()] - 80.0).abs() < 1e-3);
    assert!((hour.deltas[OverviewSeries::Grokbot.index()] - 4.0).abs() < 1e-3);
}

fn fraction(activity: f64, max: f64) -> f64 {
    bar_height(activity, max, TRACK) / TRACK
}

/// Bar height is proportional to activity over the day's maximum, zero for no
/// activity or no maximum, and capped at the full track.
#[test]
fn test_overview_hourly_bar_height_is_proportional() {
    assert!((fraction(5.0, 10.0) - 0.5).abs() < 1e-3);
    assert!((fraction(10.0, 10.0) - 1.0).abs() < 1e-3);
    assert!(fraction(0.0, 10.0).abs() < f64::EPSILON);
    assert!(fraction(5.0, 0.0).abs() < f64::EPSILON);
    assert!((fraction(15.0, 10.0) - 1.0).abs() < 1e-3);
}

/// A provider at its own day peak keeps a full-height share next to a larger
/// provider at half its peak, so small providers stay visible.
#[test]
fn test_overview_hourly_relative_stack_keeps_small_providers_visible() {
    let grok = fraction(10.0, 20.0);
    let go = fraction(0.1, 0.1);
    assert!((grok - 0.5).abs() < 1e-3);
    assert!((go - 1.0).abs() < 1e-3);
    assert!((grok + go - 1.5).abs() < 1e-3);
    assert!(go > grok);
}
