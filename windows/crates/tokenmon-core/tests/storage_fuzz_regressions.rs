//! Stable-toolchain replay of the storage fuzz corpus and named regression inputs.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tokenmon_core::ManualClock;
use tokenmon_core::storage::{
    DailyQuotaDeltaStore, HourlyDeltaActivityStore, InMemoryStringStore, TextStore,
    decode_daily_payload, decode_hourly_payload,
};

fn corpus(target: &str) -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn exercise(text: &str) {
    let now = Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap();
    let clock = Arc::new(ManualClock::new(now));
    if let Some(p) = decode_hourly_payload(text) {
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(decode_hourly_payload(&json), Some(p));
    }
    if let Some(p) = decode_daily_payload(text) {
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(decode_daily_payload(&json), Some(p));
    }
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(text, "k");
    let mut h = HourlyDeltaActivityStore::new(backing.clone(), "k", clock.clone());
    h.record(10.0, now);
    h.record(20.0, now);
    let mut d = DailyQuotaDeltaStore::new(backing, "k", clock);
    d.record(10.0, now, None, 7);
    d.record(20.0, now, None, 7);
}

/// Every seed and any saved corpus input loads and round-trips without panicking.
#[test]
fn test_corpus_inputs_never_panic() {
    for target in ["core_storage_hourly", "core_storage_daily"] {
        for text in corpus(target) {
            exercise(&text);
        }
    }
}

/// Regression (fuzz crash-2516f905): serde_json without `float_roundtrip` parsed a
/// 15-digit fractional date 1 ulp off, so decode -> encode -> decode disagreed.
#[test]
fn test_regress_float_roundtrip_in_daily_payload() {
    let raw =
        r#"{"dayTorcent":35,"windowStart":406546906546961.556,"windowPesetsAt":813698722.556}"#;
    exercise(raw);
    let p = decode_daily_payload(raw).unwrap();
    let again = decode_daily_payload(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(p, again);
}

/// Structurally odd JSON (arrays, scalars, extremes, NaN-like text) is rejected or handled.
#[test]
fn test_odd_json_inputs_never_panic() {
    for text in [
        "",
        "null",
        "42",
        "[]",
        "{}",
        "[1,2,3,4]",
        "\"x\"",
        r#"{"dayStart":1e308,"hourWeights":[1e308]}"#,
        r#"{"dayTotals":{"2026-10-10":1e308},"windowStart":-1e308}"#,
        r#"{"days":[1,2,3],"windowResetsAt":9e99}"#,
        r#"{"dayTotals":{"0000-00-00":1,"9999-99-99":2,"-1--1--1":3}}"#,
    ] {
        exercise(text);
    }
}
