//! Stable-toolchain replays of the cargo-fuzz targets (`windows/fuzz`):
//! every committed corpus seed runs through the same invariants, plus named
//! regression inputs for hostile shapes (deep nesting, overflowing sums,
//! absurd timestamps).

mod common;

use std::path::PathBuf;

use chrono::DateTime;
use tokenmon_core::net::{json_object, looks_like_html};
use tokenmon_core::{ManualClock, ProviderError, ProviderErrorContext};
use tokenmon_provider_cursor::{
    aggregate_cost_stats, daily_tokens, estimated_weight_by_day, hour_weights, parse_email,
    parse_summary, parse_usage_events_page, quota_hour_weights, reject_unauthorized_body,
};

fn corpus(target: &str) -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../fuzz/corpus/{target}"));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| std::fs::read(e.path()).ok())
        .collect()
}

fn check_summary(data: &[u8]) {
    let now = DateTime::from_timestamp(1_786_000_000, 0).expect("fixed instant");
    if let Ok(snap) = parse_summary(data, now) {
        assert!((0.0..=100.0).contains(&snap.used_percent));
        assert_eq!(snap.pools[0].used_percent, snap.used_percent);
        assert!(
            snap.pools
                .iter()
                .all(|p| (0.0..=100.0).contains(&p.used_percent))
        );
        assert!(snap.plan_used_usd.is_none_or(f64::is_finite));
        assert!(snap.plan_limit_usd.is_none_or(f64::is_finite));
    }
    assert!(parse_email(data).is_none_or(|e| e.contains('@')));
}

fn check_events(data: &[u8]) {
    let Ok(page) = parse_usage_events_page(data) else {
        return;
    };
    assert!(page.events.len() <= page.page_count);
    let clock = ManualClock::from_unix(1_786_000_000);
    let Some(first) = page.events.first() else {
        return;
    };
    for e in &page.events {
        assert!(e.date.timestamp() > 0);
        assert!(e.weight().is_finite() && e.weight() > 0.0);
    }
    let stats = aggregate_cost_stats(&page.events, first.date);
    assert!(stats.metered_cycle_usd.is_finite());
    for w in hour_weights(&clock, &page.events, first.date)
        .into_iter()
        .chain(quota_hour_weights(
            &clock,
            &page.events,
            first.date,
            Some(40.0),
        ))
    {
        assert!(w.is_finite() && w >= 0.0);
    }
    assert!(
        daily_tokens(&clock, &page.events)
            .values()
            .all(|d| d.cost_usd.is_none_or(f64::is_finite))
    );
    assert!(
        estimated_weight_by_day(&clock, &page.events, first.date, None, first.date)
            .values()
            .all(|w| w.is_finite())
    );
}

fn check_body(data: &[u8]) {
    let ctx = ProviderErrorContext::Cursor;
    match json_object(data, ctx) {
        Ok(_) | Err(ProviderError::BadResponse(..)) => {}
        Err(ProviderError::Unauthorized(_)) => assert!(looks_like_html(data)),
        Err(other) => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        reject_unauthorized_body(data, ctx),
        Ok(_) | Err(ProviderError::Unauthorized(_) | ProviderError::BadResponse(..))
    ));
}

/// Every committed seed satisfies the summary invariants.
#[test]
fn test_summary_corpus_replays_cleanly() {
    for data in corpus("cursor_summary") {
        check_summary(&data);
    }
}

/// Every committed seed satisfies the events invariants.
#[test]
fn test_events_corpus_replays_cleanly() {
    for data in corpus("cursor_events_page") {
        check_events(&data);
    }
}

/// Every committed seed satisfies the body-classification invariants.
#[test]
fn test_body_corpus_replays_cleanly() {
    for data in corpus("cursor_http_body") {
        check_body(&data);
    }
}

/// Charged cents near `f64::MAX` must not sum to infinity.
#[test]
fn test_regression_cost_sum_overflow_stays_finite() {
    let ev = r#"{"timestamp":"1775418973898","model":"m","chargedCents":1.7e308,"requestsCosts":1.7e308}"#;
    let body = format!(r#"{{"usageEventsDisplay":[{ev},{ev},{ev}]}}"#);
    check_events(body.as_bytes());
}

/// Absurd timestamps are skipped, not converted into out-of-range dates.
#[test]
fn test_regression_absurd_timestamps_are_skipped() {
    let body = br#"{"usageEventsDisplay":[{"timestamp":"1e300"},{"timestamp":9.9e18},{"timestamp":"inf"},{"timestamp":"nan"}]}"#;
    let page = parse_usage_events_page(body).unwrap();
    assert_eq!((page.page_count, page.events.len()), (4, 0));
}

/// Deeply nested wrappers neither overflow the stack nor panic.
#[test]
fn test_regression_deep_nesting_is_rejected_cleanly() {
    let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(parse_summary(deep.as_bytes(), DateTime::from_timestamp(0, 0).unwrap()).is_err());
    assert!(parse_usage_events_page(deep.as_bytes()).is_err());
    let wrapped = format!("{}5{}", r#"{"val":"#.repeat(100), "}".repeat(100));
    check_summary(format!(r#"{{"individualUsage":{{"plan":{{"used":{wrapped}}}}}}}"#).as_bytes());
}

/// Binary garbage and non-UTF-8 prefixes are classified, never panicked on.
#[test]
fn test_regression_binary_bodies_are_total() {
    for body in [&[0xff_u8, 0xfe, 0x00][..], b"", b"\x00<", b"   <html>"] {
        check_body(body);
        check_summary(body);
        check_events(body);
    }
}
