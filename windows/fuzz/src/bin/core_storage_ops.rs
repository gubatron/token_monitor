//! Fuzzes operation sequences against the hourly and daily stores.
//!
//! Invariants: no panic; every total is finite and non-negative; a reload of the
//! persisted state agrees with the live store (round trip through the JSON).
#![no_main]

use std::sync::Arc;

use arbitrary::Arbitrary;
use chrono::{Duration, TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ManualClock;
use tokenmon_core::storage::{
    DailyQuotaDeltaStore, HourlyDeltaActivityStore, InMemoryStringStore, QuotaWindow,
};

#[derive(Arbitrary, Debug)]
enum Op {
    Record {
        pct: u16,
        hours: u16,
        start: Option<u16>,
        reset: Option<u16>,
    },
    BeginWindow,
    Clear,
}

fuzz_target!(|ops: Vec<Op>| {
    let base = Utc.with_ymd_and_hms(2026, 10, 10, 0, 0, 0).unwrap();
    let clock = Arc::new(ManualClock::new(base));
    let hb = Arc::new(InMemoryStringStore::default());
    let db = Arc::new(InMemoryStringStore::default());
    let mut hourly = HourlyDeltaActivityStore::new(hb, "h", clock.clone());
    let mut daily = DailyQuotaDeltaStore::new(db.clone(), "d", clock.clone());
    let mut latest = base;
    for op in ops.iter().take(256) {
        match op {
            Op::Record {
                pct,
                hours,
                start,
                reset,
            } => {
                let at = base + Duration::hours(i64::from(*hours));
                latest = latest.max(at);
                let used = f64::from(*pct) / 100.0;
                let window = QuotaWindow {
                    start: start.map(|s| base + Duration::hours(i64::from(s))),
                    resets_at: reset.map(|r| base + Duration::hours(i64::from(r))),
                };
                hourly.record(used, at);
                daily.record(used, at, Some(window), 7);
            }
            Op::BeginWindow => {
                hourly.begin_new_window(false);
                daily.begin_new_window(None, None);
            }
            Op::Clear => {
                hourly.clear(latest);
                daily.clear();
            }
        }
        assert!(
            hourly
                .hour_weights()
                .iter()
                .all(|w| w.is_finite() && *w >= 0.0)
        );
        assert!(
            daily
                .day_totals()
                .values()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
    }
    let reloaded = DailyQuotaDeltaStore::new(db, "d", clock);
    assert_eq!(reloaded.day_totals(), daily.day_totals());
});
