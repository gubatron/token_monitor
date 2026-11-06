//! Ports of `OverviewHourlyHistoryTests` (history, series, OpenCode) and
//! `ProviderHourlyUsageTests` (padding), plus codec robustness cases.

use chrono::NaiveDate;
use tokenmon_core::overview::{
    CursorDay, DayHourly, OpenCodeDay, OverviewDaySeries, OverviewHistory,
    OverviewSeries::{self, Claude, Cursor, Grok, OpenCodeGo, OpenCodeZen},
    ProviderDayHourlyUsage, TodayInputs, build_today_series,
};

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).expect("valid date")
}

fn hours(pairs: &[(usize, f64)]) -> Vec<f64> {
    let mut v = vec![0.0; 24];
    for (h, w) in pairs {
        v[*h] = *w;
    }
    v
}

/// OpenCode hours are added when the data is for the same day.
#[test]
fn test_open_code_hours_are_added_for_the_same_day() {
    let day = d(2026, 10, 22);
    let oc = OpenCodeDay {
        day,
        go_weights: hours(&[(9, 1.5)]),
        zen_weights: hours(&[(10, 0.5)]),
        go_tokens: (0..24).collect(),
        zen_tokens: vec![7; 24],
    };
    let inputs = TodayInputs {
        open_code: Some(oc),
        ..Default::default()
    };
    let s = build_today_series(&inputs, day);
    assert_eq!(s.weights[&OpenCodeGo][9], 1.5);
    assert_eq!(s.weights[&OpenCodeZen][10], 0.5);
    assert_eq!(s.tokens[&OpenCodeGo][5], 5);
    assert_eq!(s.tokens[&OpenCodeZen][23], 7);
}

/// OpenCode data from another day, or without 24 hours, is ignored.
#[test]
fn test_open_code_hours_from_another_day_are_ignored() {
    let day = d(2026, 10, 22);
    let mk = |day, n| OpenCodeDay {
        day,
        go_weights: vec![1.0; n],
        zen_weights: vec![1.0; 24],
        go_tokens: vec![1; 24],
        zen_tokens: vec![1; 24],
    };
    for oc in [mk(d(2026, 10, 21), 24), mk(day, 23)] {
        let s = build_today_series(
            &TodayInputs {
                open_code: Some(oc),
                ..Default::default()
            },
            day,
        );
        assert!(!s.has_activity());
        assert!(s.tokens.is_empty());
    }
}

/// Grok, Claude and Grokbot need the same day and exactly 24 values; Cursor
/// splits into weights and tokens.
#[test]
fn test_provider_hourly_sources_follow_the_same_day_24_rule() {
    let day = d(2026, 10, 22);
    let hourly = |day, n| {
        Some(DayHourly {
            day,
            weights: vec![1.0; n],
        })
    };
    let inputs = TodayInputs {
        grok: hourly(day, 24),
        claude: hourly(day, 25),
        grokbot: hourly(d(2026, 10, 21), 24),
        cursor: Some(CursorDay {
            day,
            quota_weights: vec![2.0; 24],
            hour_tokens: vec![3; 10],
        }),
        open_code: None,
    };
    let s = build_today_series(&inputs, day);
    assert!(s.weights.contains_key(&Grok));
    assert!(!s.weights.contains_key(&Claude));
    assert!(!s.weights.contains_key(&OverviewSeries::Grokbot));
    assert!(s.weights.contains_key(&Cursor));
    assert!(!s.tokens.contains_key(&Cursor));
}

/// Non-finite provider values never reach the series.
#[test]
fn test_non_finite_weights_are_zeroed() {
    let day = d(2026, 10, 22);
    let mut w = vec![1.0; 24];
    w[0] = f64::NAN;
    w[1] = f64::INFINITY;
    let s = build_today_series(
        &TodayInputs {
            grok: Some(DayHourly { day, weights: w }),
            ..Default::default()
        },
        day,
    );
    assert_eq!(&s.weights[&Grok][..3], &[0.0, 0.0, 1.0]);
}

/// Recording keeps the larger value for each hour.
#[test]
fn test_record_keeps_the_larger_value_for_each_hour() {
    let day = d(2026, 10, 22);
    let mut h = OverviewHistory::default();
    let a = OverviewDaySeries::default().with_weights(Grok, hours(&[(1, 5.0), (2, 1.0)]));
    let b = OverviewDaySeries::default()
        .with_weights(Grok, hours(&[(1, 2.0), (2, 4.0)]))
        .with_weights(Claude, hours(&[(3, 1.0)]));
    assert!(h.record(&a, day));
    assert!(h.record(&b, day));
    let got = h.series(day).expect("day saved");
    assert_eq!(&got.weights[&Grok][1..3], &[5.0, 4.0]);
    assert_eq!(got.weights[&Claude][3], 1.0);
    // Re-recording the same data changes nothing.
    assert!(!h.record(&b, day));
}

/// An all-zero series does not create a day, but may update an existing one.
#[test]
fn test_an_empty_day_is_not_saved() {
    let day = d(2026, 10, 22);
    let mut h = OverviewHistory::default();
    let zero = OverviewDaySeries::default().with_weights(Grok, vec![0.0; 24]);
    assert!(!h.record(&zero, day));
    assert!(h.is_empty());
    assert_eq!(h.earliest_day(), None);
}

/// The history survives an encode, decode cycle through a real file.
#[test]
fn test_history_reloads_from_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("activity_overview_hourly_history.dat");
    let day = d(2026, 10, 22);
    let mut h = OverviewHistory::default();
    h.record(
        &OverviewDaySeries::default()
            .with_weights(Grok, hours(&[(9, 1.25)]))
            .with_tokens(Cursor, vec![5; 24]),
        day,
    );
    h.save(&path).expect("save");
    assert_eq!(OverviewHistory::load(&path), h);
    assert_eq!(
        OverviewHistory::load(&dir.path().join("missing")),
        OverviewHistory::default()
    );
}

/// Days older than 30 (counting today) are dropped: Oct 31 drops Oct 1.
#[test]
fn test_days_past_the_retention_window_are_dropped() {
    let mut h = OverviewHistory::default();
    let s = OverviewDaySeries::default().with_weights(Grok, hours(&[(1, 1.0)]));
    h.record(&s, d(2026, 10, 1));
    h.record(&s, d(2026, 10, 2));
    assert!(h.record(&s, d(2026, 10, 31)));
    assert_eq!(h.earliest_day(), Some(d(2026, 10, 2)));
    assert_eq!(h.len(), 2);
}

/// Trimming alone counts as a change even when the series merges to nothing new.
#[test]
fn test_trim_reports_changed_without_a_merge() {
    let mut h = OverviewHistory::default();
    let s = OverviewDaySeries::default().with_weights(Grok, hours(&[(1, 1.0)]));
    h.record(&s, d(2026, 10, 1));
    assert!(h.record(&OverviewDaySeries::default(), d(2026, 12, 1)));
    assert!(h.is_empty());
}

/// Keys are stored by name in sorted order with the version tag.
#[test]
fn test_series_keys_are_saved_by_name_and_sorted() {
    let mut h = OverviewHistory::default();
    let s = OverviewDaySeries::default()
        .with_weights(OpenCodeZen, vec![1.0])
        .with_weights(Grok, vec![1.0])
        .with_weights(OpenCodeGo, vec![1.0])
        .with_tokens(Cursor, vec![2]);
    h.record(&s, d(2026, 10, 22));
    assert_eq!(
        h.encode(),
        r#"{"days":{"2026-10-22":{"tokens":{"cursor":[2]},"weights":{"grok":[1.0],"openCodeGo":[1.0],"openCodeZen":[1.0]}}},"version":1}"#
    );
}

/// A file in the spec's documented shape decodes.
#[test]
fn test_decodes_the_documented_file_shape() {
    let text = r#"{"days":{"2026-10-22":{"tokens":{"cursor":[0,3]},"weights":{"claude":[0,1],"grok":[2]}}},"version":1}"#;
    let h = OverviewHistory::decode(text);
    let s = h.series(d(2026, 10, 22)).expect("decoded");
    assert_eq!(s.weights[&Claude], vec![0.0, 1.0]);
    assert_eq!(s.tokens[&Cursor], vec![0, 3]);
    assert_eq!(h.earliest_day(), Some(d(2026, 10, 22)));
}

/// Any decode failure gives an empty history.
#[test]
fn test_corrupt_input_decodes_to_empty() {
    for text in [
        "",
        "not json",
        "[]",
        "{}",
        r#"{"days":5}"#,
        r#"{"days":{"2026-10-22":{"weights":{"mystery":[1]}}}}"#,
        r#"{"days":{"2026-10-22":{"weights":{"grok":["x"]}}}}"#,
    ] {
        assert!(OverviewHistory::decode(text).is_empty(), "{text:?}");
    }
}

/// Non-canonical keys are dropped; oversized arrays are truncated; bad numbers
/// are zeroed.
#[test]
fn test_decode_normalizes_untrusted_content() {
    let long: Vec<String> = (0..100).map(|_| "-1".to_owned()).collect();
    let text = format!(
        r#"{{"days":{{"2026-13-01":{{}},"2026-1-1":{{}},"2026-10-22":{{"weights":{{"grok":[{}]}}}}}}}}"#,
        long.join(",")
    );
    let h = OverviewHistory::decode(&text);
    assert_eq!(h.len(), 1);
    let w = &h.series(d(2026, 10, 22)).expect("kept").weights[&Grok];
    assert_eq!(w.len(), 24);
    assert!(w.iter().all(|v| *v == 0.0));
}

/// Merging pads shorter arrays and keeps one-sided series.
#[test]
fn test_merging_pads_and_keeps_one_sided_series() {
    let a = OverviewDaySeries::default().with_weights(Grok, vec![1.0, 9.0]);
    let b = OverviewDaySeries::default()
        .with_weights(Grok, vec![4.0])
        .with_tokens(Cursor, vec![1, 2, 3]);
    let m = a.merging(&b);
    assert_eq!(m.weights[&Grok], vec![4.0, 9.0]);
    assert_eq!(m.tokens[&Cursor], vec![1, 2, 3]);
}

/// Short input arrays are padded to 24 hours.
#[test]
fn test_short_arrays_are_padded_to_twenty_four_hours() {
    let s = OverviewDaySeries::default().with_weights(Grok, vec![1.0, 2.0]);
    let u = ProviderDayHourlyUsage::build(d(2026, 10, 22), &s).expect("activity");
    assert_eq!(u.hours.len(), 24);
    assert_eq!(u.hours[1].activity, 2.0);
    assert_eq!(u.hours[23].activity, 0.0);
}

/// Long input arrays are truncated to 24 hours.
#[test]
fn test_long_arrays_are_truncated_to_twenty_four_hours() {
    let s = OverviewDaySeries::default().with_weights(Grok, vec![1.0; 40]);
    let u = ProviderDayHourlyUsage::build(d(2026, 10, 22), &s).expect("activity");
    assert_eq!(u.hours.len(), 24);
    assert_eq!(u.max_activity, 1.0);
}

/// Shares, deltas and tokens follow section 4.5.
#[test]
fn test_hour_shares_and_tokens() {
    let s = OverviewDaySeries::default()
        .with_weights(Grok, hours(&[(5, 1.0)]))
        .with_weights(OpenCodeGo, hours(&[(5, 3.0)]))
        .with_weights(Claude, hours(&[(5, -4.0)]))
        .with_tokens(OpenCodeGo, vec![-5; 24])
        .with_tokens(Cursor, vec![9; 24])
        .with_tokens(Grok, vec![100; 24]);
    let u = ProviderDayHourlyUsage::build(d(2026, 10, 22), &s).expect("activity");
    let h = &u.hours[5];
    assert_eq!(h.activity, 4.0);
    assert_eq!(h.shares[Grok.index()], 25.0);
    assert_eq!(h.shares[OpenCodeGo.index()], 75.0);
    assert_eq!(h.deltas[Claude.index()], 0.0);
    assert_eq!(h.tokens[OpenCodeGo.index()], Some(0));
    assert_eq!(h.tokens[Cursor.index()], Some(9));
    assert_eq!(h.tokens[Grok.index()], None);
}
