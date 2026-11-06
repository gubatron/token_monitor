//! Billing-period window and daily-bar tests (spec 02 sections 5.1 and 5.6).
//! All run in UTC unless a test says otherwise.
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

mod common;

use chrono::{DateTime, Datelike, TimeDelta, Utc, Weekday};
use common::{clock_at, clock_with_offset, utc};
use tokenmon_provider_grok::calendar::{DayCalendar, LocalCalendar};
use tokenmon_provider_grok::daily::{billing_period_week_bounds, preview, week};
use tokenmon_provider_grok::models::{
    DailyUsageDay, DailyUsageWeek, ProductUsage, WeeklyUsageSnapshot,
};

fn product(id: &str, pct: f64) -> ProductUsage {
    let name = match id {
        "build" => "Grok Build",
        "chat" => "Chat",
        "api" => "API",
        "voice" => "Voice",
        _ => "Other",
    };
    ProductUsage::new(id, name, pct)
}

fn snap(
    at: DateTime<Utc>,
    used: f64,
    resets: Option<DateTime<Utc>>,
    products: &[(&str, f64)],
) -> WeeklyUsageSnapshot {
    WeeklyUsageSnapshot::new(at, used)
        .with_resets_at(resets)
        .with_products(products.iter().map(|(i, p)| product(i, *p)).collect())
}

fn build_week(
    history: &[WeeklyUsageSnapshot],
    offset: i64,
    resets_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DailyUsageWeek> {
    let clock = clock_at(now);
    let cal = LocalCalendar::new(clock.as_ref());
    week(history, history.last(), offset, resets_at, &cal, now)
}

fn day_on(w: &DailyUsageWeek, at: DateTime<Utc>) -> Option<&DailyUsageDay> {
    let clock = clock_at(at);
    let cal = LocalCalendar::new(clock.as_ref());
    w.days.iter().find(|d| cal.same_day(d.day_start, at))
}

fn bounds(
    resets: DateTime<Utc>,
    offset: i64,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let clock = clock_at(now);
    let cal = LocalCalendar::new(clock.as_ref());
    billing_period_week_bounds(Some(resets), offset, &cal, now).unwrap()
}

fn assert_near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} not within {tolerance} of {expected}"
    );
}

fn span_days(start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    (end - start).num_days()
}

/// Reset Thu 07-16T18:57Z: the window is Thu 9 to Wed 15 mid-period, runs
/// through the reset day on its morning, and rolls to a single new Thursday
/// after the reset fires.
#[test]
fn test_billing_period_week_starts_on_period_start_day() {
    let reset = utc(2026, 7, 16, 18, 57);
    let (start, end) = bounds(reset, 0, utc(2026, 7, 15, 12, 0));
    assert_eq!(
        (start.weekday(), end.weekday()),
        (Weekday::Thu, Weekday::Wed)
    );
    assert_eq!((start.day(), end.day(), span_days(start, end)), (9, 15, 6));

    let morning = utc(2026, 7, 16, 12, 0);
    let (start, end) = bounds(reset, 0, morning);
    assert_eq!((start.day(), end.day(), span_days(start, end)), (9, 16, 7));

    let (start, end) = bounds(reset, -1, morning);
    assert_eq!((start.day(), end.day()), (2, 8));

    let after = utc(2026, 7, 16, 20, 0);
    let (start, end) = bounds(reset, 0, after);
    assert_eq!((start.day(), end.day(), span_days(start, end)), (16, 22, 6));
    let thursdays = (0..7)
        .filter(|d| (start + TimeDelta::days(*d)).weekday() == Weekday::Thu)
        .count();
    assert_eq!(thursdays, 1);

    let (start, end) = bounds(utc(2026, 7, 23, 18, 57), 0, after);
    assert_eq!((start.day(), end.day()), (16, 22));

    let (start, end) = bounds(reset, -1, utc(2026, 7, 15, 12, 0));
    assert_eq!((start.day(), end.day()), (2, 8));
}

/// A reset that lags more than one period rolls forward by whole periods.
#[test]
fn test_stale_reset_rolls_forward_to_contain_today() {
    let now = utc(2026, 7, 27, 12, 0);
    let (start, end) = bounds(utc(2026, 7, 16, 18, 57), 0, now);
    assert_eq!((start.day(), end.day(), span_days(start, end)), (23, 29, 6));
    let today = utc(2026, 7, 27, 0, 0);
    assert!(start <= today && end >= today);
}

/// An absurdly stale reset terminates (520-iteration cap) with a 7-day window.
#[test]
fn test_roll_forward_is_capped() {
    let (start, end) = bounds(utc(2000, 1, 1, 0, 0), 0, utc(2026, 7, 27, 12, 0));
    assert_eq!(span_days(start, end), 6);
}

/// No provider reset means no chart, not a calendar week.
#[test]
fn test_weekly_chart_refused_without_resets_at() {
    let now = utc(2026, 7, 15, 12, 0);
    let history = [snap(
        now - TimeDelta::hours(1),
        40.0,
        None,
        &[("chat", 40.0)],
    )];
    assert!(build_week(&history, 0, None, now).is_none());
}

/// `current.resets_at` stands in for a missing explicit reset.
#[test]
fn test_current_reset_is_the_fallback() {
    let now = utc(2026, 7, 15, 12, 0);
    let history = [snap(now, 40.0, Some(utc(2026, 7, 16, 18, 57)), &[])];
    assert!(build_week(&history, 0, None, now).is_some());
}

/// Yesterday 10 (build 7, chat 3), today 30 (build 20, chat 10): today is the
/// 20-point delta in two segments, yesterday is the baseline.
#[test]
fn test_daily_usage_builder_deltas() {
    let now = utc(2026, 7, 15, 12, 0);
    let reset = utc(2026, 7, 16, 18, 57);
    let history = [
        snap(
            utc(2026, 7, 14, 12, 0),
            10.0,
            Some(reset),
            &[("build", 7.0), ("chat", 3.0)],
        ),
        snap(
            utc(2026, 7, 15, 10, 0),
            30.0,
            Some(reset),
            &[("build", 20.0), ("chat", 10.0)],
        ),
    ];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    assert_eq!(w.days.len(), 7);
    assert!(w.has_daily_data);
    assert!(!w.is_estimated);
    assert_near(
        day_on(&w, utc(2026, 7, 14, 0, 0)).unwrap().total_percent(),
        0.0,
        0.2,
    );
    let today = day_on(&w, utc(2026, 7, 15, 0, 0)).unwrap();
    assert_near(today.total_percent(), 20.0, 0.2);
    assert_eq!(today.segments.len(), 2);
    assert!(today.is_today);
}

/// Only products that grew get a segment.
#[test]
fn test_daily_usage_only_shows_products_that_grew() {
    let now = utc(2026, 7, 12, 12, 0);
    let reset = utc(2026, 7, 13, 9, 0);
    let history = [
        snap(
            utc(2026, 7, 11, 18, 0),
            39.0,
            None,
            &[("chat", 23.0), ("build", 13.0), ("api", 3.0)],
        ),
        snap(
            utc(2026, 7, 12, 10, 0),
            42.0,
            None,
            &[("chat", 26.0), ("build", 13.0), ("api", 3.0)],
        ),
    ];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    let today = day_on(&w, now).unwrap();
    assert_near(today.total_percent(), 3.0, 0.2);
    assert_eq!(today.segments.len(), 1);
    assert_eq!(today.segments[0].product_id, "chat");
}

/// Flat usage across the day rollover leaves both bars empty.
#[test]
fn test_daily_usage_shows_yesterday_after_day_rollover() {
    let now = utc(2026, 7, 12, 10, 0);
    let reset = utc(2026, 7, 13, 9, 0);
    let products = [("chat", 23.0), ("build", 16.0)];
    let history = [
        snap(utc(2026, 7, 11, 20, 0), 39.0, None, &products),
        snap(utc(2026, 7, 12, 9, 0), 39.0, None, &products),
    ];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    assert_near(
        day_on(&w, utc(2026, 7, 11, 0, 0)).unwrap().total_percent(),
        0.0,
        0.2,
    );
    assert_near(day_on(&w, now).unwrap().total_percent(), 0.0, 0.2);
    assert!(!w.is_estimated);
    assert!(!w.has_daily_data);
}

/// A single sample is a baseline only: no bars, estimated.
#[test]
fn test_daily_usage_empty_until_second_sample_day() {
    let now = utc(2026, 7, 11, 18, 0);
    let reset = utc(2026, 7, 16, 18, 57);
    let history = [snap(
        now,
        39.0,
        Some(reset),
        &[("chat", 23.0), ("build", 16.0)],
    )];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    assert!(!w.has_daily_data);
    assert!(w.is_estimated);
    assert!(w.days.iter().all(|d| d.segments.is_empty()));
}

/// Build stays flat at 16 while chat, api and voice grow: today has no build.
#[test]
fn test_daily_usage_excludes_flat_build_from_today() {
    let now = utc(2026, 7, 13, 12, 0);
    let reset = utc(2026, 7, 16, 18, 57);
    let r = Some(reset);
    let history = [
        snap(
            utc(2026, 7, 11, 20, 0),
            46.0,
            r,
            &[("chat", 27.0), ("build", 16.0), ("api", 3.0)],
        ),
        snap(
            utc(2026, 7, 12, 14, 0),
            51.0,
            r,
            &[("chat", 31.0), ("build", 16.0), ("api", 4.0)],
        ),
        snap(
            utc(2026, 7, 13, 8, 0),
            58.0,
            r,
            &[
                ("chat", 34.0),
                ("build", 16.0),
                ("api", 7.0),
                ("voice", 1.0),
            ],
        ),
    ];
    let w = build_week(&history, 0, r, now).unwrap();
    let today = day_on(&w, now).unwrap();
    assert_near(today.total_percent(), 7.0, 0.2);
    let has = |id: &str| today.segments.iter().any(|s| s.product_id == id);
    assert!(!has("build") && has("chat") && has("api") && has("voice"));
}

/// Used% drops with the same reset: the recalibration day restarts tracking.
#[test]
fn test_daily_usage_mid_period_recalibration_rebases_onto_that_day() {
    let now = utc(2026, 7, 14, 22, 0);
    let r = Some(utc(2026, 7, 16, 18, 57));
    let history = [
        snap(
            utc(2026, 7, 12, 18, 0),
            51.0,
            r,
            &[("chat", 31.0), ("build", 16.0), ("api", 4.0)],
        ),
        snap(
            utc(2026, 7, 13, 12, 0),
            71.0,
            r,
            &[
                ("chat", 34.0),
                ("build", 28.0),
                ("api", 7.0),
                ("voice", 2.0),
            ],
        ),
        snap(
            utc(2026, 7, 14, 16, 0),
            28.0,
            r,
            &[("chat", 1.0), ("build", 20.0), ("api", 7.0)],
        ),
    ];
    let w = build_week(&history, 0, r, now).unwrap();
    let today = day_on(&w, now).unwrap();
    assert!(today.is_after_reset);
    assert_near(
        day_on(&w, utc(2026, 7, 12, 0, 0)).unwrap().total_percent(),
        0.0,
        0.2,
    );
    assert_near(
        day_on(&w, utc(2026, 7, 13, 0, 0)).unwrap().total_percent(),
        0.0,
        0.2,
    );
    assert_near(today.total_percent(), 28.0, 0.5);
    assert!(today.segments.iter().any(|s| s.product_id == "build"));
    assert!(w.is_estimated);
}

/// Real rollover (used drops and the reset advances): the new total is
/// attributed to that day, flagged after-reset.
#[test]
fn test_daily_usage_real_reset_shows_post_reset_usage() {
    let now = utc(2026, 7, 15, 20, 0);
    let old = utc(2026, 7, 16, 18, 57);
    let new = utc(2026, 7, 23, 18, 57);
    let history = [
        snap(utc(2026, 7, 14, 12, 0), 90.0, Some(old), &[("build", 90.0)]),
        snap(
            utc(2026, 7, 15, 8, 0),
            12.0,
            Some(new),
            &[("chat", 8.0), ("api", 4.0)],
        ),
    ];
    let w = build_week(&history, 0, Some(old), now).unwrap();
    let today = day_on(&w, now).unwrap();
    assert!(today.is_after_reset);
    assert_near(today.total_percent(), 12.0, 0.2);
    assert!(today.segments.iter().any(|s| s.product_id == "chat"));
}

/// A sample from the prior billing period adds no bar.
#[test]
fn test_daily_usage_ignores_prior_billing_period_sample() {
    let now = utc(2026, 7, 13, 12, 0);
    let reset = utc(2026, 7, 16, 18, 57);
    let history = [
        snap(
            utc(2026, 7, 8, 18, 0),
            90.0,
            Some(utc(2026, 7, 9, 18, 57)),
            &[("build", 90.0)],
        ),
        snap(
            utc(2026, 7, 13, 10, 0),
            20.0,
            Some(reset),
            &[("chat", 12.0), ("build", 8.0)],
        ),
    ];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    assert!(w.days.iter().all(|d| d.segments.is_empty()));
    assert!(w.is_estimated);
}

/// The new period's first day (a Thursday) shows period-to-date usage.
#[test]
fn test_period_start_thursday_shows_usage() {
    let now = utc(2026, 7, 16, 20, 0);
    let reset = utc(2026, 7, 23, 18, 57);
    let history = [snap(
        now,
        12.0,
        Some(reset),
        &[("chat", 8.0), ("build", 4.0)],
    )];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    assert_eq!(w.week_start.weekday(), Weekday::Thu);
    let thursday = day_on(&w, now).unwrap();
    assert_near(thursday.total_percent(), 12.0, 0.5);
    assert!(!thursday.segments.is_empty());
}

/// A cross-reset pair does not invent a 90 to 8 drop bar.
#[test]
fn test_cross_reset_deltas_do_not_invent_drop_bar() {
    let now = utc(2026, 7, 17, 15, 0);
    let (wed, fri) = (utc(2026, 7, 15, 18, 0), utc(2026, 7, 17, 12, 0));
    let new = utc(2026, 7, 23, 18, 57);
    let history = [
        snap(
            wed,
            90.0,
            Some(utc(2026, 7, 16, 18, 57)),
            &[("build", 90.0)],
        ),
        snap(fri, 8.0, Some(new), &[("chat", 8.0)]),
    ];
    let w = build_week(&history, 0, Some(new), now).unwrap();
    assert!(day_on(&w, wed).is_none());
    let friday = day_on(&w, fri).unwrap();
    assert_near(friday.total_percent(), 0.0, 0.5);
    assert!(friday.segments.iter().all(|s| s.percent_of_weekly <= 50.0));
    assert!(w.is_estimated);
}

/// Browsing back after the rollover paints the prior period's local deltas.
#[test]
fn test_previous_week_keeps_prior_period_daily_usage_after_rollover() {
    let now = utc(2026, 7, 17, 15, 0);
    let old = Some(utc(2026, 7, 16, 18, 57));
    let new = utc(2026, 7, 23, 18, 57);
    let history = [
        snap(utc(2026, 7, 13, 18, 0), 40.0, old, &[("build", 40.0)]),
        snap(utc(2026, 7, 14, 18, 0), 55.0, old, &[("build", 55.0)]),
        snap(utc(2026, 7, 15, 18, 0), 70.0, old, &[("build", 70.0)]),
        snap(utc(2026, 7, 17, 12, 0), 8.0, Some(new), &[("chat", 8.0)]),
    ];
    let w = build_week(&history, -1, Some(new), now).unwrap();
    assert_eq!((w.week_start.day(), w.week_end.day()), (9, 15));
    assert_near(
        day_on(&w, utc(2026, 7, 14, 0, 0)).unwrap().total_percent(),
        15.0,
        0.5,
    );
    assert_near(
        day_on(&w, utc(2026, 7, 15, 0, 0)).unwrap().total_percent(),
        15.0,
        0.5,
    );
    assert!(w.has_daily_data);
}

/// Usage before the reset instant on the reset day lands on the eighth bar.
#[test]
fn test_reset_day_usage_before_reset_instant_is_charted() {
    let reset = utc(2026, 7, 16, 18, 57);
    let thu_morning = utc(2026, 7, 16, 12, 0);
    let history = [
        snap(
            utc(2026, 7, 15, 20, 0),
            60.0,
            Some(reset),
            &[("chat", 60.0)],
        ),
        snap(thu_morning, 72.0, Some(reset), &[("chat", 72.0)]),
    ];
    let w = build_week(&history, 0, Some(reset), thu_morning).unwrap();
    assert_eq!(w.days.len(), 8);
    let last = w.days.last().unwrap();
    assert!(last.is_today);
    assert_near(last.total_percent(), 12.0, 0.5);
}

/// An early provider reset keeps the old window's days on the previous page.
#[test]
fn test_early_reset_keeps_old_window_history_on_the_previous_page() {
    let old = utc(2026, 10, 7, 18, 5);
    let new = utc(2026, 10, 9, 18, 5);
    let now = utc(2026, 10, 3, 15, 0);
    let s = |at, used, resets| snap(at, used, Some(resets), &[("chat", used)]);
    let history = [
        s(utc(2026, 9, 30, 12, 0), 10.0, old),
        s(utc(2026, 10, 1, 12, 0), 30.0, old),
        s(utc(2026, 10, 2, 20, 0), 3.0, new),
        s(utc(2026, 10, 3, 12, 0), 8.0, new),
    ];
    let current = build_week(&history, 0, Some(new), now).unwrap();
    assert_eq!(current.week_start.day(), 2);
    let oct2 = current
        .days
        .iter()
        .find(|d| d.day_start.day() == 2)
        .unwrap();
    assert_near(oct2.total_percent(), 3.0, 0.5);
    let previous = build_week(&history, -1, Some(new), now).unwrap();
    let oct1 = previous
        .days
        .iter()
        .find(|d| d.day_start.day() == 1)
        .unwrap();
    assert_near(oct1.total_percent(), 20.0, 0.5);
}

/// The legend lists each product once, in display order.
#[test]
fn test_legend_is_unique_and_in_display_order() {
    let now = utc(2026, 7, 15, 12, 0);
    let reset = utc(2026, 7, 16, 18, 57);
    let history = [
        snap(utc(2026, 7, 13, 12, 0), 5.0, Some(reset), &[("api", 5.0)]),
        snap(
            utc(2026, 7, 14, 12, 0),
            15.0,
            Some(reset),
            &[("api", 8.0), ("chat", 7.0)],
        ),
        snap(
            utc(2026, 7, 15, 10, 0),
            25.0,
            Some(reset),
            &[("api", 12.0), ("chat", 10.0), ("build", 3.0)],
        ),
    ];
    let w = build_week(&history, 0, Some(reset), now).unwrap();
    let ids: Vec<&str> = w.legend_products.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["chat", "build", "api"]);
}

/// A near-duplicate current sample (same day, within 60 s, same used%) is not
/// appended twice.
#[test]
fn test_current_near_duplicate_is_not_double_counted() {
    let now = utc(2026, 7, 15, 12, 0);
    let reset = utc(2026, 7, 16, 18, 57);
    let last = snap(now, 30.0, Some(reset), &[("chat", 30.0)]);
    let current = snap(
        now + TimeDelta::seconds(30),
        30.01,
        Some(reset),
        &[("chat", 30.0)],
    );
    let clock = clock_at(now);
    let cal = LocalCalendar::new(clock.as_ref());
    let history = [
        snap(
            utc(2026, 7, 14, 12, 0),
            10.0,
            Some(reset),
            &[("chat", 10.0)],
        ),
        last,
    ];
    let w = week(&history, Some(&current), 0, Some(reset), &cal, now).unwrap();
    assert_near(day_on(&w, now).unwrap().total_percent(), 20.0, 0.2);
}

/// The preview is a Thursday-to-Wednesday week of seven bars.
#[test]
fn test_daily_usage_preview_has_seven_days() {
    let clock = clock_at(utc(2026, 1, 1, 0, 0));
    let cal = LocalCalendar::new(clock.as_ref());
    let w = preview(&cal);
    assert_eq!(w.days.len(), 7);
    assert!(w.has_daily_data);
    assert!(!w.is_estimated);
    assert_eq!(w.week_start.weekday(), Weekday::Thu);
    assert_eq!(w.week_end.weekday(), Weekday::Wed);
    assert_eq!(w.days[0].weekday_symbol, "Thu");
    assert_eq!(w.days[0].day_of_month, "9");
}

/// Day boundaries follow the local offset: with UTC-8, a 03:00Z sample belongs
/// to the previous local day.
#[test]
fn test_local_offset_defines_day_boundaries() {
    let now = utc(2026, 7, 15, 20, 0);
    let clock = clock_with_offset(now, -8);
    let cal = LocalCalendar::new(clock.as_ref());
    let sample = utc(2026, 7, 15, 3, 0); // 19:00 local on Jul 14
    assert_eq!(cal.start_of_day(sample), utc(2026, 7, 14, 8, 0));
    assert!(!cal.same_day(sample, now));
    let reset = utc(2026, 7, 17, 2, 0); // Jul 16 18:00 local
    let (start, end) = billing_period_week_bounds(Some(reset), 0, &cal, now).unwrap();
    assert_eq!(start, utc(2026, 7, 9, 8, 0));
    assert_eq!(end, utc(2026, 7, 15, 8, 0));
}

/// Extreme reset, offset and sample dates never panic and still yield a
/// bounded window.
#[test]
fn test_extreme_dates_do_not_panic() {
    let clock = clock_at(DateTime::<Utc>::MAX_UTC);
    let cal = LocalCalendar::new(clock.as_ref());
    for reset in [
        DateTime::<Utc>::MIN_UTC,
        DateTime::<Utc>::MAX_UTC,
        utc(1970, 1, 1, 0, 0),
    ] {
        for now in [DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC] {
            for offset in [i64::MIN, -1, 0, 1, i64::MAX] {
                if let Some((s, e)) = billing_period_week_bounds(Some(reset), offset, &cal, now) {
                    assert!(s <= e || offset != 0);
                }
            }
        }
    }
    let extreme = [snap(
        DateTime::<Utc>::MAX_UTC,
        50.0,
        Some(DateTime::<Utc>::MIN_UTC),
        &[],
    )];
    let _ = week(
        &extreme,
        extreme.last(),
        0,
        Some(DateTime::<Utc>::MAX_UTC),
        &cal,
        DateTime::<Utc>::MAX_UTC,
    );
}
