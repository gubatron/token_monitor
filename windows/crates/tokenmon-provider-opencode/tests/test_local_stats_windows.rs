//! More `OpenCodeStatsTests` ports: local-stats flags, merging with console
//! data, BYOK exclusion and subscription-window aggregation (spec 03 section 7.7).
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
use tokenmon_provider_opencode::bounds::{Bounds, monthly_bounds};
use tokenmon_provider_opencode::db::{ScanCache, load_scan, scan_start};
use tokenmon_provider_opencode::snapshot::{OpenCodeSnapshot, WindowKind};
use tokenmon_provider_opencode::stats::{daily_spends_by_day, go_rows_in, snapshot};

const NOW: i64 = 1_785_592_600;
const H: i64 = 3600;
const D: i64 = 86_400;

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("ts")
}

fn scan_of(f: &Fixture) -> std::sync::Arc<tokenmon_provider_opencode::db::Scan> {
    load_scan(&f.path, scan_start(at(NOW)), &ScanCache::default()).expect("scan")
}

fn assert_close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

/// Console-only snapshots have no local stats; a model makes them true.
#[test]
fn test_has_local_stats_reflects_local_contribution() {
    assert!(!OpenCodeSnapshot::default().has_local_stats());
    let mut f = Fixture::new();
    f.insert("g", Msg::go("", NOW - H, "minimax-m3", 1.0), false);
    assert!(snapshot(&scan_of(&f), at(NOW)).has_local_stats());
}

/// Console windows survive a merge; local models and monthly totals are taken when present.
#[test]
fn test_merge_local_models_keeps_console_windows() {
    let mut f = Fixture::new();
    f.insert(
        "g",
        Msg::go("", NOW - H, "minimax-m3", 1.0).tokens([4, 1, 0, 0]),
        false,
    );
    let local = snapshot(&scan_of(&f), at(NOW));
    let console = tokenmon_provider_opencode::console::parse_go_meters(
        br#"{"access":{"meters":{"month":{"limitMicroCents":"6000000000","usedMicroCents":"0"}}}}"#,
    )
    .unwrap()
    .into_snapshot();
    let merged = console.clone().merged_with_local(Some(&local));
    assert_eq!(merged.windows, console.windows);
    assert_eq!(merged.models, local.models);
    assert_eq!(merged.monthly_tokens, 5);
    assert!(!merged.is_estimated);
    assert_eq!(console.clone().merged_with_local(None), console);
}

/// Bring-your-own-key providers never reach the model list or Go limits.
#[test]
fn test_byok_providers_excluded_from_models() {
    let mut f = Fixture::new();
    f.insert("g", Msg::go("", NOW - H, "minimax-m3", 2.0), false);
    for (i, p) in ["deepseek", "xai", "openai", "anthropic"]
        .into_iter()
        .enumerate()
    {
        f.insert(
            &format!("b{i}"),
            Msg::go("", NOW - H, "m", 5.0).provider(p),
            false,
        );
    }
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert!(
        snap.models
            .iter()
            .all(|m| m.provider_id == "opencode-go" || m.provider_id == "opencode")
    );
    assert_close(snap.window(WindowKind::Rolling5h).unwrap().used_usd, 2.0);
}

/// Subscription window: pre-month spend is excluded from the monthly window.
#[test]
fn test_monthly_aggregation_uses_subscription_window() {
    let mut f = Fixture::new();
    let month = monthly_bounds(at(NOW), at(NOW - 40 * D));
    f.insert(
        "anchor",
        Msg::go("", NOW - 40 * D, "minimax-m3", 0.0),
        false,
    );
    f.insert(
        "old",
        Msg::go("", month.start.timestamp() - H, "minimax-m3", 20.0),
        false,
    );
    f.insert(
        "a",
        Msg::go("", month.start.timestamp() + H, "minimax-m3", 7.0),
        false,
    );
    f.insert("b", Msg::go("", NOW - H, "minimax-m3", 3.0), false);
    let snap = snapshot(&scan_of(&f), at(NOW));
    let m = snap.window(WindowKind::Monthly).unwrap();
    assert_close(m.used_usd, 10.0);
    assert_eq!(m.session_count, 2);
}

/// Monthly tokens and the estimated value sum plan rows in the month.
#[test]
fn test_monthly_tokens_and_estimated_value() {
    let mut f = Fixture::new();
    f.insert(
        "g",
        Msg::go("", NOW - H, "minimax-m3", 0.0).tokens([1_000_000, 1_000_000, 0, 0]),
        false,
    );
    let snap = snapshot(&scan_of(&f), at(NOW));
    assert!(snap.monthly_tokens >= 2_000_000);
    assert_close(snap.monthly_estimated_usd, 0.30 + 1.20);
}

/// Daily spends over an explicit window keep only Go rows inside `[from, to)`.
#[test]
fn test_fetch_daily_spends_filters_to_window() {
    let mut f = Fixture::new();
    let day = |d| {
        Utc.with_ymd_and_hms(2026, 9, d, 12, 0, 0)
            .unwrap()
            .timestamp()
    };
    f.session("s", day(1), "opencode-go", "minimax-m3", false);
    for (d, c) in [(10, 5.0), (16, 3.0), (17, 9.0)] {
        f.message(Msg::go("s", day(d), "minimax-m3", c));
    }
    let now = Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap();
    let scan = load_scan(&f.path, scan_start(now), &ScanCache::default()).unwrap();
    let window = Bounds {
        start: Utc.with_ymd_and_hms(2026, 9, 16, 0, 0, 0).unwrap(),
        end: Utc.with_ymd_and_hms(2026, 9, 18, 0, 0, 0).unwrap(),
    };
    let utc = FixedOffset::east_opt(0).unwrap();
    let spends = daily_spends_by_day(&go_rows_in(&scan, window), utc);
    assert_eq!(spends.len(), 2);
    assert_close(spends.values().sum(), 12.0);
}
