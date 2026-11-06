#![no_main]
//! Crafted `opencode.db` files: arbitrary bytes written to a file and opened
//! read-only through the scanner must never panic. A scan that opens must
//! yield sane rows (non-empty ids, dates inside the supported range) and the
//! local estimate, hourly stacks and month budget built from it must stay
//! finite and in range.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::usage::{Calendar, WeekOffset};
use tokenmon_provider_opencode::budget::{MonthBudgetRequest, month_daily_budget_days};
use tokenmon_provider_opencode::db::{MAX_ROW_MS, ScanCache, load_scan, scan_start};
use tokenmon_provider_opencode::stats::{build_day_hourly_usage, snapshot};

/// One scratch file per fuzzing process, rewritten on every input.
fn scratch_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        std::env::temp_dir().join(format!("tokenmon-fuzz-opencode-{}.db", std::process::id()))
    })
}

fuzz_target!(|data: &[u8]| {
    let path = scratch_path();
    if std::fs::write(path, data).is_err() {
        return;
    }
    let now = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let Ok(scan) = load_scan(path, scan_start(now), &ScanCache::default()) else {
        return;
    };
    for row in &scan.rows {
        assert!(!row.provider_id.is_empty() && !row.model_id.is_empty());
        assert!(row.date.timestamp_millis() <= MAX_ROW_MS);
        assert!(row.cost_usd.is_finite());
    }
    let snap = snapshot(&scan, now);
    for w in &snap.windows {
        let p = w.used_percent();
        assert!(p.is_finite() && (0.0..=100.0).contains(&p));
    }
    let _ = build_day_hourly_usage(&scan.rows, now, &Calendar::utc(), 8);
    let load = move |_| Ok(Arc::clone(&scan));
    let request = MonthBudgetRequest {
        limit_usd: 60.0,
        used_percent: snap.monthly_used_percent(),
        period_resets_at: Some(Utc.with_ymd_and_hms(2026, 9, 5, 0, 0, 0).unwrap()),
        week_offset: WeekOffset::CURRENT,
        now,
        spent_by_day: None,
    };
    if let Some(budget) = month_daily_budget_days(&request, &load, &Calendar::utc()) {
        assert!(budget.days.iter().all(|d| d.spent_usd.is_finite()));
    }
});
