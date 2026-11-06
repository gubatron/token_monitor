//! Stable-toolchain regression tests for inputs the fuzz targets cover
//! (`opencode_console`, `opencode_model_name`, `opencode_bounds`) and for
//! crafted-database edge cases.
#![allow(
    clippy::unreadable_literal,
    clippy::unwrap_used,
    clippy::default_trait_access,
    clippy::redundant_closure_for_method_calls,
    clippy::assert_is_empty
)]

mod common;

use chrono::{DateTime, Utc};
use common::{Fixture, Msg};
use tokenmon_provider_opencode::bounds::{monthly_bounds, weekly_bounds};
use tokenmon_provider_opencode::console::{
    parse_go_meters, parse_orgs, parse_session_email, usd, workspace_id_from_path,
};
use tokenmon_provider_opencode::db::{ScanCache, load_scan, scan_start};
use tokenmon_provider_opencode::model_name::{display_name, palette_index};
use tokenmon_provider_opencode::pricing::model_id_variants;
use tokenmon_provider_opencode::stats::{build_day_hourly_usage, snapshot};

/// Week bounds at the extreme ends of chrono's range must not overflow.
#[test]
fn test_regression_weekly_bounds_at_range_extremes() {
    for now in [DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC] {
        let w = weekly_bounds(now);
        assert!(w.start <= now && now <= w.end);
    }
}

/// Month bounds with out-of-range anchors fall back instead of panicking.
#[test]
fn test_regression_monthly_bounds_at_range_extremes() {
    let extremes = [
        DateTime::<Utc>::MIN_UTC,
        DateTime::<Utc>::MAX_UTC,
        DateTime::UNIX_EPOCH,
    ];
    for now in extremes {
        for sub in extremes {
            let m = monthly_bounds(now, sub);
            assert!(m.start <= m.end);
            assert!((m.end - m.start).num_days() <= 31);
        }
    }
}

/// Hostile numerics in meters never yield a NaN or out-of-range percent.
#[test]
fn test_regression_hostile_meter_numbers() {
    let body = br#"{"access":{"endsAt":"nope","meters":{
        "fiveHour":{"limitMicroCents":"nan","usedMicroCents":"1e400"},
        "week":{"limitMicroCents":"-5","usedMicroCents":"-5"},
        "month":{"limitMicroCents":0,"usedMicroCents":9e300}}}}"#;
    let snap = parse_go_meters(body).unwrap().into_snapshot();
    assert_eq!(snap.windows.len(), 3);
    for w in &snap.windows {
        let p = w.used_percent();
        assert!(p.is_finite() && (0.0..=100.0).contains(&p), "{p}");
    }
    assert_eq!(usd(&serde_json::json!("-inf")), None);
}

/// Deeply nested and truncated JSON is rejected, not a stack overflow.
#[test]
fn test_regression_deep_and_truncated_json() {
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    assert!(parse_go_meters(deep.as_bytes()).is_err());
    assert!(parse_orgs(deep.as_bytes()).is_err());
    assert_eq!(parse_session_email(deep.as_bytes()), None);
    assert!(parse_go_meters(br#"{"access":{"meters":{"month":"#).is_err());
    assert!(parse_go_meters(b"").is_err());
}

/// Workspace id extraction handles odd paths.
#[test]
fn test_regression_workspace_paths() {
    assert_eq!(
        workspace_id_from_path("//wrk_//"),
        None.or(Some("wrk_".to_owned()))
    );
    assert_eq!(workspace_id_from_path(""), None);
}

/// Name normaliser on empty, separator-only and non-ASCII input.
#[test]
fn test_regression_model_name_odd_inputs() {
    for s in [
        "",
        "-",
        "--",
        "\u{1F600}-\u{0301}",
        "GPT-",
        "gpt-5-",
        "gpt--5",
        "1-2-3",
        "ß-ß",
    ] {
        let _ = display_name(s);
        assert!(palette_index(s) < 8);
        let v = model_id_variants(s);
        assert!(v.windows(2).all(|w| w[1].len() < w[0].len()));
    }
    assert_eq!(display_name("1-2-3"), "1.2.3");
    assert_eq!(display_name("gpt--5"), "GPT-5");
}

/// A row with a timestamp beyond year 9999 is skipped instead of overflowing later date maths.
#[test]
fn test_regression_far_future_row_is_skipped() {
    let mut f = Fixture::new();
    f.session("s", 1_700_000_000, "opencode-go", "minimax-m3", false);
    f.message_json(
        "s",
        i64::MAX / 1000,
        r#"{"role":"assistant","providerID":"opencode-go","modelID":"m","cost":1}"#,
    );
    f.conn
        .execute(
            "INSERT INTO message VALUES ('x','s',?1,?1,?2)",
            rusqlite::params![
                i64::MAX,
                r#"{"role":"assistant","providerID":"opencode-go","modelID":"m","cost":1}"#
            ],
        )
        .unwrap();
    f.message(Msg::go("s", 1_700_000_100, "minimax-m3", 1.0));
    let now = DateTime::<Utc>::from_timestamp(1_700_000_200, 0).unwrap();
    let scan = load_scan(&f.path, scan_start(now), &ScanCache::default()).unwrap();
    assert_eq!(scan.rows.len(), 1);
    let _ = snapshot(&scan, now);
    let off = chrono::FixedOffset::east_opt(14 * 3600).unwrap();
    let cal = tokenmon_core::usage::Calendar::fixed(off);
    let _ = build_day_hourly_usage(&scan.rows, now, &cal, 8);
}
