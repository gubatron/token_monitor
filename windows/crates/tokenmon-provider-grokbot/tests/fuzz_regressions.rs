//! Stable-toolchain replay of the `tokenmon_provider_grokbot_parse_usage_status`
//! fuzz corpus plus named regressions, asserting the fuzz target's invariants.

#![allow(clippy::unwrap_used)]

use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::ProviderError;
use tokenmon_core::usage::Calendar;
use tokenmon_provider_grokbot::parse_usage_status;

fn now() -> DateTime<Utc> {
    Utc.timestamp_opt(1_787_000_000, 0).unwrap()
}

/// The harness invariants for one input.
fn check(data: &[u8]) {
    match parse_usage_status(data, Some("user@example.com"), now()) {
        Ok(s) => {
            assert!((0.0..=100.0).contains(&s.used_percent));
            assert!((0.0..=100.0).contains(&s.remaining_percent()));
            let cal = Calendar::utc();
            assert!(s.days_in_period(&cal) >= 1);
            let _ = s.usage_pool(&cal);
        }
        Err(
            ProviderError::Unauthorized(_)
            | ProviderError::BadResponse(..)
            | ProviderError::Custom { .. },
        ) => {}
        Err(other) => panic!("unexpected error kind: {other:?}"),
    }
}

/// Every committed corpus input satisfies the invariants.
#[test]
fn test_corpus_inputs_satisfy_fuzz_invariants() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus/tokenmon_provider_grokbot_parse_usage_status");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        check(&std::fs::read(entry.unwrap().path()).unwrap());
        count += 1;
    }
    assert!(count >= 10, "corpus unexpectedly small: {count}");
}

/// Hand-picked hostile shapes: non-finite and overflowing timestamp math,
/// deep wrappers, empty and binary bodies.
#[test]
fn test_hostile_inputs_never_panic() {
    let deep_open = "{\"val\":".repeat(5_000);
    let deep = format!("{{\"usagePercent\":{deep_open}1{}}}", "}".repeat(5_000));
    let cases: [&[u8]; 9] = [
        b"",
        b"\x00\xff",
        b"[]",
        b"null",
        br#"{"usagePercent":"NaN","nextResetTimestampUtc":{"seconds":"inf"}}"#,
        br#"{"usagePercent":1,"nextResetTimestampUtc":{"seconds":1e18,"nanos":-1e30}}"#,
        br#"{"usagePercent":1,"nextResetTimestampUtc":{"seconds":1,"nanos":-1e30}}"#,
        br#"{"usagePercent":1e999}"#,
        br#"{"error":123,"usagePercent":true}"#,
    ];
    for case in cases {
        check(case);
    }
    check(deep.as_bytes());
}

/// A huge negative `nanos` once could push the instant below zero; it must be
/// handled without panicking and never yield a reset in the far past.
#[test]
fn test_negative_nanos_do_not_underflow() {
    let s = parse_usage_status(
        br#"{"usagePercent":1,"nextResetTimestampUtc":{"seconds":5,"nanos":-9e9}}"#,
        None,
        now(),
    )
    .unwrap();
    assert!(s.resets_at.is_none_or(|r| r.timestamp() >= -10));
}
