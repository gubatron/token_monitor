//! Invariant checks shared by the fuzz targets (`windows/fuzz`) and the
//! stable-toolchain regression tests, so both assert the same thing.
//!
//! Each `check_*` must never panic on any input and asserts the spec 03
//! section 8 invariants: used >= 0 and finite, percent in `[0, 100]` or
//! `None`, shares sum to at most 100, no NaN anywhere.

#![allow(dead_code, clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use tokenmon_provider_openrouter::{
    CreditsData, KeyData, NoCostEstimator, OpenRouterSnapshot, parse_activity, parse_credits,
    parse_key,
};

fn assert_snapshot_invariants(s: &OpenRouterSnapshot) {
    assert!(
        s.used_usd.is_finite() && s.used_usd >= 0.0,
        "used {}",
        s.used_usd
    );
    if let Some(p) = s.used_percent() {
        assert!(p.is_finite() && (0.0..=100.0).contains(&p), "percent {p}");
    }
    if s.budget_usd.is_none() {
        assert!(s.used_percent().is_none());
    }
    let mut last = f64::INFINITY;
    let mut sum = 0.0;
    for m in &s.models {
        assert!(m.cost_usd.is_finite() && m.cost_usd >= 0.0);
        assert!(m.percent_of_window.is_finite() && (0.0..=100.0).contains(&m.percent_of_window));
        assert!(m.requests >= 0 && m.prompt_tokens >= 0 && m.completion_tokens >= 0);
        assert!(m.cost_usd <= last, "models not sorted by cost");
        last = m.cost_usd;
        sum += m.percent_of_window;
    }
    assert!(sum <= 100.0 + 1e-6, "shares sum {sum}");
}

fn now() -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000, 0).unwrap()
}

fn base_key() -> KeyData {
    parse_key(br#"{"data":{"usage":1}}"#).unwrap()
}

fn build(key: &KeyData, credits: Option<CreditsData>) -> OpenRouterSnapshot {
    OpenRouterSnapshot::build(key, credits, None, now(), &NoCostEstimator)
}

/// `/key` bytes: parse, then build with every reset window and with and
/// without credits.
pub fn check_key(data: &[u8]) {
    let Ok(key) = parse_key(data) else { return };
    let credits = [
        None,
        Some(CreditsData {
            total_credits: 120.0,
            total_usage: 30.0,
        }),
    ];
    for reset in [
        None,
        Some("daily"),
        Some("WEEKLY"),
        Some("monthly"),
        Some("x"),
    ] {
        let mut k = key.clone();
        if let Some(r) = reset {
            k.limit_reset = Some(r.to_owned());
        }
        for c in credits {
            assert_snapshot_invariants(&build(&k, c));
        }
    }
}

/// `/credits` bytes: parse, then build against a plain key.
pub fn check_credits(data: &[u8]) {
    let Ok(credits) = parse_credits(data) else {
        return;
    };
    assert_snapshot_invariants(&build(&base_key(), Some(credits)));
    let mut limited = base_key();
    limited.limit = Some(10.0);
    assert_snapshot_invariants(&build(&limited, Some(credits)));
}

/// `/activity` bytes: parse, group, check ordering and shares.
pub fn check_activity(data: &[u8]) {
    let Ok(rows) = parse_activity(data) else {
        return;
    };
    let s = OpenRouterSnapshot::build(&base_key(), None, Some(&rows), now(), &NoCostEstimator);
    assert_snapshot_invariants(&s);
}
