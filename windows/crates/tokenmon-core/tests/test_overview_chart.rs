//! Ports of the chart tests in `OverviewHourlyHistoryTests` and
//! `testMidnightScheduleStartsAtTodayThenEachLocalMidnight`.

use chrono::{NaiveDate, Timelike};
use tokenmon_core::overview::format::{format_tokens, hour_label};
use tokenmon_core::overview::{
    ChartModel, OverviewDaySeries,
    OverviewSeries::{self, Claude, Cursor, Grok, Grokbot, OpenCodeGo, OpenCodeZen},
    ProviderDayHourlyUsage, TRACK_HEIGHT, axis_hours, bar_height, can_go_back, day_for_offset,
    day_title, empty_message, midnight_schedule, next_offset, previous_offset, segment_heights,
    series_color, show_navigation, tooltip_text, visible_hours,
};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).expect("valid date")
}

fn at(pairs: &[(usize, f64)]) -> Vec<f64> {
    let mut v = vec![0.0; 24];
    for (h, w) in pairs {
        v[*h] = *w;
    }
    v
}

fn usage(s: &OverviewDaySeries) -> ProviderDayHourlyUsage {
    ProviderDayHourlyUsage::build(d(2026, 10, 22), s).expect("activity")
}

/// [1,3] over 40 gives 10 and 30.
#[test]
fn test_segment_heights_are_proportional_and_sum_to_the_bar() {
    let h = segment_heights(&[1.0, 3.0], 40.0, 1.0);
    assert_eq!(h, vec![10.0, 30.0]);
    assert!((h.iter().sum::<f64>() - 40.0).abs() < 1e-9);
}

/// Thin slices are lifted to 1 without overflowing the bar.
#[test]
fn test_thin_slices_are_lifted_without_overflowing_the_bar() {
    let h = segment_heights(&[0.1, 0.1, 19.8], 20.0, 1.0);
    assert!((h[0] - 1.0).abs() < 1e-9 && (h[1] - 1.0).abs() < 1e-9);
    assert!((h[2] - 18.0).abs() < 1e-9);
    assert!((h.iter().sum::<f64>() - 20.0).abs() < 1e-9);
}

/// When the minimums do not fit, slices stay proportional.
#[test]
fn test_slices_stay_proportional_when_the_bar_is_too_short_for_minimums() {
    let h = segment_heights(&[1.0, 1.0, 2.0], 2.0, 1.0);
    assert_eq!(h, vec![0.5, 0.5, 1.0]);
}

/// No activity or no height gives zeros; negatives count as zero.
#[test]
fn test_segment_heights_degenerate_inputs() {
    assert_eq!(segment_heights(&[0.0, -1.0], 10.0, 1.0), vec![0.0, 0.0]);
    assert_eq!(segment_heights(&[1.0, 2.0], 0.0, 1.0), vec![0.0, 0.0]);
    assert_eq!(segment_heights(&[], 10.0, 1.0), Vec::<f64>::new());
    assert_eq!(
        segment_heights(&[f64::NAN, 2.0], 10.0, 1.0),
        vec![0.0, 10.0]
    );
}

/// Bar heights: 30, 60, 3 (minimum) and 0.
#[test]
fn test_bar_height_follows_activity_with_a_visible_minimum() {
    assert_eq!(bar_height(5.0, 10.0, 60.0), 30.0);
    assert_eq!(bar_height(10.0, 10.0, 60.0), 60.0);
    assert_eq!(bar_height(0.01, 10.0, 60.0), 3.0);
    assert_eq!(bar_height(0.0, 10.0, 60.0), 0.0);
    assert_eq!(bar_height(20.0, 10.0, 60.0), 60.0);
}

/// Visible hours widen for early and late activity.
#[test]
fn test_visible_hours_widen_to_include_late_and_early_activity() {
    assert_eq!(visible_hours(None), 6..=22);
    assert_eq!(visible_hours(Some((9, 15))), 6..=22);
    assert_eq!(visible_hours(Some((2, 15))), 2..=22);
    assert_eq!(visible_hours(Some((9, 23))), 6..=23);
    assert_eq!(visible_hours(Some((0, 23))), 0..=23);
}

/// Axis labels match the default and widened days and never crowd.
#[test]
fn test_axis_labels_match_the_default_day_and_never_crowd() {
    assert_eq!(axis_hours(&(6..=22)), vec![6, 9, 12, 15, 18, 22]);
    assert_eq!(axis_hours(&(2..=23)), vec![2, 6, 9, 12, 15, 18, 21, 23]);
    for lo in 0..=6u8 {
        for hi in 22..=23u8 {
            let hs = axis_hours(&(lo..=hi));
            assert!(
                hs.windows(2).all(|w| w[1] - w[0] >= 2),
                "{lo}..={hi}: {hs:?}"
            );
        }
    }
}

/// Titles for today, yesterday and older days; empty messages.
#[test]
fn test_day_titles() {
    let day = d(2026, 10, 19);
    assert_eq!(day_title(0, day), "Usage Today");
    assert_eq!(day_title(-1, day), "Usage Yesterday");
    assert_eq!(day_title(-3, day), "Usage Mon, Oct 19");
    assert_eq!(day_title(-3, d(2026, 10, 5)), "Usage Mon, Oct 5");
    assert!(empty_message(0).starts_with("No Grok, Grokbot, OpenCode, Cursor, or Claude"));
    assert_eq!(empty_message(-2), "No activity recorded on this day.");
}

/// Back needs an earlier saved day; Next stops at today.
#[test]
fn test_back_arrow_needs_an_earlier_saved_day() {
    let today = d(2026, 10, 22);
    assert!(!can_go_back(today, None));
    assert!(!can_go_back(today, Some(today)));
    assert!(can_go_back(today, Some(d(2026, 10, 21))));
    assert!(!show_navigation(false, 0));
    assert!(show_navigation(true, 0));
    assert!(show_navigation(false, -2));
    assert_eq!(previous_offset(0), -1);
    assert_eq!(next_offset(-2), -1);
    assert_eq!(next_offset(-1), 0);
    assert_eq!(next_offset(0), 0);
    assert_eq!(day_for_offset(today, -2), d(2026, 10, 20));
}

/// Token and hour label formatting from the spec's examples.
#[test]
fn test_format_examples() {
    for (v, s) in [
        (999, "999"),
        (1_000, "1K"),
        (1_500, "2K"),
        (2_500, "3K"),
        (999_499, "999K"),
        (999_500, "1.0M"),
        (1_234_567, "1.2M"),
        (1_250_000, "1.3M"),
        (9_949_999, "9.9M"),
        (9_950_000, "10M"),
        (123_000_000, "123M"),
        (999_499_999, "999M"),
        (999_500_000, "1.0B"),
        (2_500_000_000, "2.5B"),
    ] {
        assert_eq!(format_tokens(v), s, "{v}");
    }
    assert_eq!(hour_label(0), "12a");
    assert_eq!(hour_label(1), "1a");
    assert_eq!(hour_label(11), "11a");
    assert_eq!(hour_label(12), "12p");
    assert_eq!(hour_label(13), "1p");
    assert_eq!(hour_label(23), "11p");
}

/// Tooltips list slices in stack order with quota labels and tokens.
#[test]
fn test_tooltip_text() {
    let s = OverviewDaySeries::default()
        .with_weights(Grok, at(&[(14, 0.5)]))
        .with_weights(Grokbot, at(&[(14, 0.25)]))
        .with_weights(Claude, at(&[(14, 1.0)]))
        .with_weights(Cursor, at(&[(14, 2.0)]))
        .with_weights(OpenCodeGo, at(&[(14, 3.0)]))
        .with_tokens(Cursor, vec![1_234_567; 24])
        .with_tokens(OpenCodeGo, vec![0; 24]);
    let u = usage(&s);
    assert_eq!(
        tooltip_text(&u.hours[14]),
        "2p: Grok 0.50% of weekly · Grokbot 0.25% of weekly · \
         Cursor 2.00% of weekly share · 1.2M · Claude 1.00% of 5-hour window · \
         OpenCode Go 3.00% of weekly share"
    );
}

/// The chart lays out bars, empty capsules, legend and tooltips.
#[test]
fn test_chart_model_for_a_default_day() {
    let s = OverviewDaySeries::default()
        .with_weights(Grok, at(&[(9, 1.0), (10, 4.0)]))
        .with_weights(OpenCodeZen, at(&[(10, 4.0)]))
        .with_weights(Claude, at(&[(3, 1.0)]));
    let chart = ChartModel::build(&usage(&s));
    assert_eq!(chart.range, 3..=22);
    assert_eq!(chart.bars.len(), 20);
    let b10 = chart.bars.iter().find(|b| b.hour == 10).expect("hour 10");
    assert_eq!(b10.height, TRACK_HEIGHT);
    assert_eq!(b10.slices.len(), 2);
    assert_eq!(b10.slices[0].series, Grok);
    assert_eq!(b10.slices[0].bottom, 0.0);
    assert_eq!(b10.slices[1].series, OpenCodeZen);
    assert_eq!(b10.slices[1].bottom, b10.slices[0].height);
    assert!((b10.slices.iter().map(|x| x.height).sum::<f64>() - TRACK_HEIGHT).abs() < 1e-9);
    let b9 = chart.bars.iter().find(|b| b.hour == 9).expect("hour 9");
    // Hour 10 has activity 8 and hour 9 has 1.
    assert_eq!(b9.height, TRACK_HEIGHT / 8.0);
    let empty = chart.bars.iter().find(|b| b.hour == 12).expect("hour 12");
    assert!(empty.empty_capsule && empty.slices.is_empty() && empty.tooltip.is_none());
    let legend: Vec<_> = chart.legend.iter().map(|l| (l.series, l.title)).collect();
    assert_eq!(
        legend,
        vec![(Grok, "Grok"), (Claude, "Claude"), (OpenCodeZen, "Zen")]
    );
}

/// The bar maximum and legend consider only visible hours.
#[test]
fn test_chart_legend_is_ordered_and_uses_short_opencode_titles() {
    let s = OverviewDaySeries::default()
        .with_weights(OpenCodeGo, at(&[(8, 1.0)]))
        .with_weights(Grokbot, at(&[(8, 1.0)]))
        .with_weights(Cursor, at(&[(8, 1.0)]));
    let chart = ChartModel::build(&usage(&s));
    let titles: Vec<_> = chart.legend.iter().map(|l| l.title).collect();
    assert_eq!(titles, ["Grokbot", "Cursor", "Go"]);
}

/// Series colors: accents at 0.85, Go orange 0.85, Zen purple 0.78.
#[test]
fn test_series_colors() {
    assert_eq!(series_color(OpenCodeGo).to_rgb8(), (255, 140, 0));
    assert_eq!(series_color(OpenCodeGo).alpha, 0.85);
    assert_eq!(series_color(OpenCodeZen).to_rgb8(), (148, 112, 219));
    assert_eq!(series_color(OpenCodeZen).alpha, 0.78);
    assert_eq!(series_color(Grok).alpha, 0.85);
    for s in OverviewSeries::STACK_ORDER {
        assert!(series_color(s).alpha > 0.0);
    }
}

/// Eight local midnights, DST-safe (Denver 2026-03-08 springs forward).
#[test]
fn test_midnight_schedule_starts_at_today_then_each_local_midnight() {
    let tz = chrono_tz::America::Denver;
    let sched = midnight_schedule(&tz, d(2026, 3, 7));
    assert_eq!(sched.len(), 8);
    for (k, t) in sched.iter().enumerate() {
        let local = t.with_timezone(&tz);
        assert_eq!((local.hour(), local.minute()), (0, 0), "entry {k}");
        assert_eq!(local.date_naive(), d(2026, 3, 7 + k as u32));
    }
    // The day of the change is 23 hours long.
    assert_eq!((sched[2] - sched[1]).num_hours(), 23);
    assert_eq!((sched[1] - sched[0]).num_hours(), 24);
}

/// A midnight that does not exist falls to the first valid instant.
#[test]
fn test_midnight_schedule_handles_a_midnight_dst_gap() {
    // Havana springs forward at 00:00 on the second Sunday of March.
    let tz = chrono_tz::America::Havana;
    let sched = midnight_schedule(&tz, d(2026, 3, 7));
    assert_eq!(sched.len(), 8);
    assert!(sched.windows(2).all(|w| w[0] < w[1]));
    let gap_day = sched[1].with_timezone(&tz);
    assert_eq!(gap_day.date_naive(), d(2026, 3, 8));
    assert_eq!(gap_day.hour(), 1);
}
