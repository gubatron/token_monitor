//! Ports of `OpenCodeLocalScanTests` and `OpenCodeStatsTests` (spec 03 section 7.7)
//! against a fixture database with the real schema.
#![allow(
    clippy::unreadable_literal,
    clippy::unwrap_used,
    clippy::default_trait_access,
    clippy::redundant_closure_for_method_calls,
    clippy::assert_is_empty
)]

mod common;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use common::{Fixture, Msg};
use tokenmon_provider_opencode::bounds::{monthly_bounds, weekly_bounds};
use tokenmon_provider_opencode::db::{DbError, ScanCache, load_scan, open_read_only, scan_start};
use tokenmon_provider_opencode::snapshot::WindowKind;
use tokenmon_provider_opencode::stats::{
    build_day_hourly_usage, daily_spends_by_day, go_rows_in, month_daily_spends,
    overview_hour_token_counts, overview_quota_hour_weights, scaled_spends_percent, snapshot,
    spread_used_percent,
};

const NOW: i64 = 1_785_592_600;
const H: i64 = 3600;
const D: i64 = 86_400;

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("ts")
}

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).expect("utc")
}

fn scan_of(f: &Fixture) -> std::sync::Arc<tokenmon_provider_opencode::db::Scan> {
    load_scan(&f.path, scan_start(at(NOW)), &ScanCache::default()).expect("scan")
}

fn assert_close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

/// A session that started before the rolling window still counts its recent messages.
#[test]
fn test_session_started_before_rolling_window_counts_recent_messages() {
    let mut f = Fixture::new();
    f.session("s1", NOW - 10 * H, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s1", NOW - 6 * H, "minimax-m3", 1.0));
    f.message(Msg::go("s1", NOW - H, "minimax-m3", 3.0));
    let snap = snapshot(&scan_of(&f), at(NOW));
    let rolling = snap.window(WindowKind::Rolling5h).expect("rolling");
    assert_close(rolling.used_usd, 3.0);
    assert_eq!(rolling.session_count, 1);
}

/// A long session splits across weeks by message time.
#[test]
fn test_long_session_splits_across_weeks_by_message_time() {
    let mut f = Fixture::new();
    let week = weekly_bounds(at(NOW));
    f.session(
        "s",
        week.start.timestamp() - 3 * D,
        "opencode-go",
        "minimax-m3",
        false,
    );
    f.message(Msg::go("s", week.start.timestamp() - H, "minimax-m3", 5.0));
    f.message(Msg::go("s", week.start.timestamp() + H, "minimax-m3", 2.0));
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert_close(
        snap.window(WindowKind::Weekly).expect("weekly").used_usd,
        2.0,
    );
}

/// Monthly totals come from the subscription month only.
#[test]
fn test_monthly_totals_come_from_the_month_only() {
    let mut f = Fixture::new();
    let week = weekly_bounds(at(NOW));
    let anchor = week.start.timestamp() + D;
    // Anchor so the month starts at week.start + 1d (use the same day-of-month).
    f.session(
        "anchor",
        anchor - 30 * D,
        "opencode-go",
        "minimax-m3",
        false,
    );
    let month = monthly_bounds(at(NOW), at(anchor - 30 * D));
    let _ = month;
    f.message(Msg::go(
        "anchor",
        week.start.timestamp() + 2 * H,
        "minimax-m3",
        40.0,
    ));
    f.message(Msg::go("anchor", NOW - H, "minimax-m3", 2.0).tokens([10, 5, 7, 3]));
    let scan = scan_of(&f);
    let snap = snapshot(&scan, at(NOW));
    let m = monthly_bounds(at(NOW), scan.subscribed_at.expect("anchor"));
    assert!(m.contains(at(NOW)));
    // Both rows are in the current subscription month here, so assert the
    // monthly sum matches a manual bounds filter.
    let expected: f64 = go_rows_in(&scan, m).iter().map(|r| r.billable().cost).sum();
    assert_close(
        snap.window(WindowKind::Monthly).expect("monthly").used_usd,
        expected,
    );
    assert_eq!(snap.monthly_tokens, 25);
    assert_eq!(snap.monthly_input_tokens, 10);
    assert_eq!(snap.monthly_output_tokens, 5);
}

/// Zero-cost Go rows are valued from tokens for the headline and the daily bars alike.
#[test]
fn test_headline_and_daily_bars_value_zero_cost_go_rows_alike() {
    let mut f = Fixture::new();
    f.session("s", NOW - 2 * H, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", NOW - H, "minimax-m3", 0.0).tokens([1_000_000, 0, 0, 0]));
    let scan = scan_of(&f);
    let snap = snapshot(&scan, at(NOW));
    let weekly = snap.window(WindowKind::Weekly).expect("weekly").used_usd;
    assert!(weekly > 0.0);
    let week = weekly_bounds(at(NOW));
    let spends = daily_spends_by_day(&go_rows_in(&scan, week), utc());
    assert_close(weekly, spends.values().sum());
}

/// Rows whose `data` is not JSON are ignored.
#[test]
fn test_malformed_message_rows_are_skipped() {
    let mut f = Fixture::new();
    f.session("s", NOW - 2 * H, "opencode-go", "minimax-m3", false);
    f.message_json("s", NOW - H, "not json");
    f.message(Msg::go("s", NOW - H, "minimax-m3", 1.0));
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert_close(snap.window(WindowKind::Rolling5h).expect("r").used_usd, 1.0);
}

/// The scan cache reuses an unchanged database and reloads on a wider window or a change.
#[test]
fn test_scan_cache_reuses_unchanged_database() {
    let mut f = Fixture::new();
    f.session("s", NOW - 2 * H, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", NOW - H, "minimax-m3", 1.0));
    let cache = ScanCache::default();
    let since = scan_start(at(NOW));
    load_scan(&f.path, since, &cache).expect("1");
    load_scan(&f.path, since, &cache).expect("same");
    load_scan(&f.path, since + chrono::Duration::days(1), &cache).expect("later");
    assert_eq!(cache.loads(), 1);
    load_scan(&f.path, since - chrono::Duration::days(1), &cache).expect("earlier");
    assert_eq!(cache.loads(), 2);
    f.message(Msg::go("s", NOW - H, "minimax-m3", 2.0));
    load_scan(&f.path, since - chrono::Duration::days(1), &cache).expect("changed");
    assert_eq!(cache.loads(), 3);
}

/// One scan feeds every consumer until the database changes.
#[test]
fn test_fetches_share_one_scan_until_the_database_changes() {
    let mut f = Fixture::new();
    f.session("s", NOW - 2 * H, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", NOW - H, "minimax-m3", 1.0));
    let cache = ScanCache::default();
    let since = scan_start(at(NOW));
    let a = load_scan(&f.path, since, &cache).expect("a");
    assert_eq!(a.rows.len(), 1);
    assert!(a.subscribed_at.is_some());
    f.message(Msg::go("s", NOW - 30 * 60, "minimax-m3", 1.0));
    let b = load_scan(&f.path, since, &cache).expect("b");
    assert_eq!(b.rows.len(), 2);
}

/// A missing database is an error, not an empty scan, and is never created.
#[test]
fn test_database_missing_throws_and_is_not_created() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("opencode.db");
    let err = load_scan(&path, at(0), &ScanCache::default()).expect_err("missing");
    assert!(matches!(err, DbError::Missing(_)));
    assert!(
        err.to_string()
            .starts_with("OpenCode usage database not found at ")
    );
    assert!(matches!(open_read_only(&path), Err(DbError::Missing(_))));
    assert!(!path.exists());
}

/// The connection is read-only: writes are refused by SQLite.
#[test]
fn test_connection_is_read_only() {
    let f = Fixture::new();
    let conn = open_read_only(&f.path).expect("open");
    assert!(conn.execute("DELETE FROM message", []).is_err());
}

/// A WAL database a writer still holds open can be read; the reader leaves it unchanged.
#[test]
fn test_reads_wal_database_while_writer_holds_it_open() {
    let mut f = Fixture::new();
    f.conn
        .query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .expect("wal");
    f.session("s", NOW - 2 * H, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", NOW - H, "minimax-m3", 4.0));
    let wal = std::path::PathBuf::from(format!("{}-wal", f.path.display()));
    assert!(wal.exists());
    let scan = scan_of(&f);
    assert_eq!(scan.rows.len(), 1);
    f.message(Msg::go("s", NOW - 60, "minimax-m3", 1.0));
    let cache = ScanCache::default();
    let a = load_scan(&f.path, scan_start(at(NOW)), &cache).expect("a");
    assert_eq!(a.rows.len(), 2);
}

/// Weird column values (text in numbers, NULLs, huge ints, empty ids) never panic.
#[test]
fn test_crafted_rows_are_coerced_or_skipped() {
    let mut f = Fixture::new();
    f.session("s", NOW - 2 * H, "opencode-go", "minimax-m3", false);
    for data in [
        r#"{"role":"assistant","providerID":"opencode-go","modelID":"minimax-m3","cost":"abc","tokens":{"input":"12","output":null}}"#,
        r#"{"role":"assistant","providerID":"","modelID":"x"}"#,
        r#"{"role":"assistant","providerID":"opencode-go","modelID":"minimax-m3","tokens":{"input":9223372036854775807,"output":9223372036854775807}}"#,
        r#"{"role":"user","providerID":"opencode-go","modelID":"minimax-m3"}"#,
        r#"{"role":"assistant","providerID":"opencode-go","modelID":"minimax-m3","cost":1e999}"#,
    ] {
        f.message_json("s", NOW - H, data);
    }
    let scan = scan_of(&f);
    assert_eq!(scan.rows.len(), 3);
    let _ = snapshot(&scan, at(NOW));
    assert_eq!(scan.rows[0].tokens.input, 12);
}

/// Archived sessions do not anchor the subscription; Go aggregation excludes other providers.
#[test]
fn test_aggregation_windows_and_models() {
    let mut f = Fixture::new();
    f.insert(
        "a",
        Msg::go("", NOW - H, "minimax-m3", 4.0).tokens([10_000_000, 1_000_000, 2_000_000, 0]),
        false,
    );
    f.insert(
        "b",
        Msg::go("", NOW - 2 * H, "minimax-m3", 2.0).tokens([5_000_000, 0, 1_000_000, 0]),
        false,
    );
    f.insert(
        "c",
        Msg::go("", NOW - 2 * D, "claude", 90.0).provider("anthropic"),
        false,
    );
    f.insert(
        "d",
        Msg::go("", NOW - 3 * H, "deepseek-v4-flash-free", 3.0).provider("opencode"),
        false,
    );
    f.insert("e", Msg::go("", NOW - 40 * D, "kimi-k2.6", 500.0), false);
    f.insert("f", Msg::go("", NOW - 40 * D, "kimi-k2.6", 99.0), true);
    let scan = scan_of(&f);
    let snap = snapshot(&scan, at(NOW));
    let rolling = snap.window(WindowKind::Rolling5h).expect("r");
    assert_close(rolling.used_usd, 6.0);
    assert_eq!(rolling.session_count, 2);
    assert_close(rolling.used_percent(), 50.0);
    let go_model = snap
        .models
        .iter()
        .find(|m| m.provider_id == "opencode-go")
        .expect("go model");
    assert_eq!(go_model.session_count, 2);
    assert_eq!(go_model.input_tokens, 15_000_000);
    assert_eq!(go_model.output_tokens, 1_000_000);
    assert_eq!(go_model.cache_read_tokens, 3_000_000);
    assert_close(go_model.cost_usd, 6.0);
    assert!(snap.models.iter().all(|m| m.provider_id != "anthropic"));
    assert!(
        snap.models
            .iter()
            .any(|m| m.model_id == "deepseek-v4-flash-free")
    );
    assert_eq!(snap.models[0].model_id, "minimax-m3");
    assert!(snap.is_estimated);
}

/// Zen spend does not count toward Go limits.
#[test]
fn test_zen_does_not_count_toward_go_limits() {
    let mut f = Fixture::new();
    f.insert(
        "z",
        Msg::go("", NOW - H, "deepseek-v4-pro", 8.0).provider("opencode"),
        false,
    );
    f.insert("g", Msg::go("", NOW - H, "minimax-m3", 1.0), false);
    let snap = snapshot(&scan_of(&f), at(NOW));
    let r = snap.window(WindowKind::Rolling5h).expect("r");
    assert_close(r.used_usd, 1.0);
    assert_eq!(r.session_count, 1);
    assert!((r.used_percent() - 8.3333).abs() < 0.001);
}

/// The rolling reset follows the last Go message; Zen is ignored.
#[test]
fn test_rolling_reset_tracks_last_eligible_session() {
    let mut f = Fixture::new();
    f.insert("g", Msg::go("", NOW - H, "minimax-m3", 1.0), false);
    f.insert(
        "z",
        Msg::go("", NOW - 600, "minimax-m3", 1.0).provider("opencode"),
        false,
    );
    let snap = snapshot(&scan_of(&f), at(NOW));
    let reset = snap.window(WindowKind::Rolling5h).expect("r").resets_at;
    assert_eq!(reset, Some(at(NOW - H + 5 * H)));
}

/// Models use the weekly window and the primary percent is monthly.
#[test]
fn test_models_use_weekly_window() {
    let mut f = Fixture::new();
    let week = weekly_bounds(at(NOW));
    let t = (week.start.timestamp() + H).min(NOW - H);
    f.insert("g", Msg::go("", t, "minimax-m3", 2.0), false);
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert_eq!(snap.models.len(), 1);
    assert!((snap.monthly_used_percent() - 2.0 / 60.0 * 100.0).abs() < 1e-6);
}

/// Window percent is clamped and a zero limit gives zero.
#[test]
fn test_clamped_percent() {
    use tokenmon_provider_opencode::snapshot::WindowUsage;
    let w = |used, limit| WindowUsage {
        kind: WindowKind::Rolling5h,
        used_usd: used,
        limit_usd: limit,
        resets_at: None,
        session_count: 0,
    };
    assert_close(w(6.0, 12.0).used_percent(), 50.0);
    assert_close(w(15.0, 12.0).used_percent(), 100.0);
    assert_close(w(0.0, 12.0).used_percent(), 0.0);
    assert_close(w(5.0, 0.0).used_percent(), 0.0);
    assert_close(w(f64::NAN, 12.0).used_percent(), 0.0);
}

/// Labels and default limits.
#[test]
fn test_window_labels_and_limits() {
    let all = [
        (WindowKind::Rolling5h, "5-Hour", 12.0),
        (WindowKind::Weekly, "Weekly", 30.0),
        (WindowKind::Monthly, "Monthly", 60.0),
    ];
    for (k, label, limit) in all {
        assert_eq!(k.label(), label);
        assert_close(k.default_limit_usd(), limit);
    }
}

/// Weekly bounds are the UTC Monday week; Sunday belongs to the earlier Monday.
#[test]
fn test_weekly_bounds_utc_monday() {
    let wed = Utc.with_ymd_and_hms(2026, 7, 15, 13, 0, 0).unwrap();
    let b = weekly_bounds(wed);
    assert_eq!(b.start.timestamp(), 1_783_900_800);
    assert_eq!((b.end - b.start).num_days(), 7);
    let sun = Utc.with_ymd_and_hms(2026, 7, 19, 23, 0, 0).unwrap();
    assert_eq!(weekly_bounds(sun).start, b.start);
}

/// Monthly bounds anchor on the subscription date.
#[test]
fn test_monthly_bounds_subscription_anchored() {
    let b = monthly_bounds(at(1_785_596_400), at(1_779_114_281));
    assert!((b.start.timestamp() - 1_784_384_681).abs() <= 2);
    assert!((b.end.timestamp() - 1_787_063_081).abs() <= 2);
}

/// Short months clamp the anchor day.
#[test]
fn test_monthly_bounds_clamps_short_months() {
    let sub = Utc.with_ymd_and_hms(2026, 1, 31, 10, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 2, 15, 0, 0, 0).unwrap();
    let b = monthly_bounds(now, sub);
    assert_eq!(b.start, sub);
    assert_eq!(b.end, Utc.with_ymd_and_hms(2026, 2, 28, 10, 0, 0).unwrap());
}

/// With the anchor later in the month than now, the window starts last month.
#[test]
fn test_monthly_bounds_example_from_spec() {
    let sub = Utc.with_ymd_and_hms(2026, 5, 18, 14, 24, 41).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 1, 15, 0, 0).unwrap();
    let b = monthly_bounds(now, sub);
    assert_eq!(
        b.start,
        Utc.with_ymd_and_hms(2026, 7, 18, 14, 24, 41).unwrap()
    );
    assert_eq!(
        b.end,
        Utc.with_ymd_and_hms(2026, 8, 18, 14, 24, 41).unwrap()
    );
}

/// With no Go session the monthly window anchors at now, so monthly spend is zero.
#[test]
fn test_monthly_without_go_session_is_zero() {
    let mut f = Fixture::new();
    f.insert(
        "z",
        Msg::go("", NOW - H, "deepseek-v4-pro", 3.0).provider("opencode"),
        false,
    );
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert_close(snap.window(WindowKind::Monthly).expect("m").used_usd, 0.0);
}

/// Scaling anchors the spends' total to the used percent.
#[test]
fn test_scaled_spends_percent() {
    let d = |n| chrono::NaiveDate::from_ymd_opt(2026, 8, n).unwrap();
    let spends = [(d(4), 5.0), (d(5), 15.0)].into_iter().collect();
    let scaled = scaled_spends_percent(&spends, 30.0);
    assert_close(scaled[&d(4)], 7.5);
    assert_close(scaled[&d(5)], 22.5);
    assert_eq!(scaled_spends_percent(&spends, 0.0), spends);
    assert!(scaled_spends_percent(&Default::default(), 10.0).is_empty());
}

/// Month spends track Go only.
#[test]
fn test_month_daily_spends_track_go_only() {
    let mut f = Fixture::new();
    f.insert("g", Msg::go("", NOW - H, "minimax-m3", 3.0), false);
    f.insert(
        "z",
        Msg::go("", NOW - H, "deepseek-v4-pro", 5.0).provider("opencode"),
        false,
    );
    f.insert(
        "d",
        Msg::go("", NOW - H, "x", 4.0).provider("deepseek"),
        false,
    );
    let spends = month_daily_spends(&scan_of(&f), at(NOW), utc());
    assert_eq!(spends.len(), 1);
    assert_close(spends.values().sum(), 3.0);
}

/// With no local spends the used percent is spread over the elapsed days.
#[test]
fn test_spread_when_no_local_spends() {
    let d = |n| chrono::NaiveDate::from_ymd_opt(2026, 8, n).unwrap();
    let bars = spread_used_percent(d(5), d(7), 12.0);
    assert_eq!(bars.len(), 3);
    assert_close(bars.values().sum(), 12.0);
    assert!(bars[&d(7)] > 0.0);
}

/// Hourly stacks group models by local hour, newest day only; legend drops yesterday.
#[test]
fn test_day_hourly_usage_stacks_models_by_local_hour() {
    let mut f = Fixture::new();
    let now = Utc.with_ymd_and_hms(2026, 8, 1, 18, 0, 0).unwrap();
    let day0 = Utc
        .with_ymd_and_hms(2026, 8, 1, 0, 0, 0)
        .unwrap()
        .timestamp();
    f.session("s", day0, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", day0 + 9 * H + 60, "minimax-m3", 2.0));
    f.message(Msg::go("s", day0 + 9 * H + 120, "kimi-k2.6", 1.0));
    f.message(Msg::go("s", day0 + 14 * H, "minimax-m3", 4.0));
    f.message(Msg::go("s", day0 - H, "old", 9.0));
    let scan = load_scan(&f.path, scan_start(now), &ScanCache::default()).expect("scan");
    let day = build_day_hourly_usage(&scan.rows, now, utc(), 8);
    assert_eq!(day.hours.len(), 24);
    let total: f64 = day.hours.iter().map(|h| h.total_usd()).sum();
    assert_close(total, 7.0);
    assert_eq!(day.hours[9].segments.len(), 2);
    assert_close(day.hours[9].total_usd(), 3.0);
    assert_close(day.hours[14].total_usd(), 4.0);
    assert!(day.hours[0].segments.is_empty());
    assert!(day.legend.iter().all(|l| l.label != "old"));
    assert_eq!(day.legend[0].label, "minimax-m3");
    assert!(!day.is_empty());
    assert_eq!(day.hours[0].label(), "12a");
    assert_eq!(day.hours[13].label(), "1p");
    assert_eq!(day.hours[12].label(), "12p");
}

/// A mid-session model switch shows up in the hourly stacks and the model list.
#[test]
fn test_hourly_and_models_track_mid_session_model_switch() {
    let mut f = Fixture::new();
    let now = Utc.with_ymd_and_hms(2026, 8, 3, 18, 0, 0).unwrap();
    let day0 = Utc
        .with_ymd_and_hms(2026, 8, 3, 0, 0, 0)
        .unwrap()
        .timestamp();
    f.session("s", day0, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", day0 + 15 * H, "gpt-5.6-luna", 0.5));
    let scan = load_scan(&f.path, scan_start(now), &ScanCache::default()).expect("scan");
    let day = build_day_hourly_usage(&scan.rows, now, utc(), 8);
    assert!(day.hours[15].total_usd() > 0.0);
    assert!(
        snapshot(&scan, now)
            .models
            .iter()
            .any(|m| m.model_id == "gpt-5.6-luna")
    );
}

/// Overview series count only plan providers.
#[test]
fn test_overview_counts_only_plan_usage() {
    let mut f = Fixture::new();
    let now = Utc.with_ymd_and_hms(2026, 8, 3, 18, 0, 0).unwrap();
    let day0 = Utc
        .with_ymd_and_hms(2026, 8, 3, 0, 0, 0)
        .unwrap()
        .timestamp();
    f.session("s", day0, "opencode-go", "minimax-m3", false);
    f.message(Msg::go("s", day0 + 10 * H, "minimax-m3", 3.0).tokens([1, 0, 0, 0]));
    f.message(
        Msg::go("s", day0 + 10 * H, "deepseek-v4-pro", 1.0)
            .provider("opencode")
            .tokens([1, 0, 0, 0]),
    );
    f.message(Msg::go("s", day0 + 11 * H, "grok", 5.0).provider("xai"));
    let scan = load_scan(&f.path, scan_start(now), &ScanCache::default()).expect("scan");
    let day = build_day_hourly_usage(&scan.rows, now, utc(), 8);
    assert_eq!(day.hours[10].segments.len(), 2);
    let w = overview_quota_hour_weights(&day, 10.0);
    let weekly = 10.0 / (365.2425 / 12.0 / 7.0);
    assert_close(w.go[10], 3.0 / weekly * 100.0);
    assert_close(w.zen[10], 1.0 / weekly * 100.0);
    assert_close(w.go[11] + w.zen[11], 0.0);
    let t = overview_hour_token_counts(&day);
    assert_close(t.go[10], 1.0);
    assert_close(t.zen[10], 1.0);
    let zero = overview_quota_hour_weights(&day, 0.0);
    assert!(zero.go.iter().chain(zero.zen.iter()).all(|v| *v == 0.0));
}

/// Models with no cost and no tokens are dropped.
#[test]
fn test_unused_models_excluded_from_list() {
    let mut f = Fixture::new();
    f.insert("g", Msg::go("", NOW - H, "minimax-m3", 1.0), false);
    f.insert("h", Msg::go("", NOW - H, "kimi-k3", 0.0), false);
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert_eq!(snap.models.len(), 1);
}

/// Unpriced models with tokens are flagged on the model row.
#[test]
fn test_unpriced_model_flagged_on_row() {
    let mut f = Fixture::new();
    f.insert(
        "g",
        Msg::go("", NOW - H, "totally-new-model", 0.0).tokens([5, 5, 0, 0]),
        false,
    );
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert!(snap.models[0].is_cost_unpriced);
}
