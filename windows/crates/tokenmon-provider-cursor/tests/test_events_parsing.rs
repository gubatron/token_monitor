//! Events parsing and aggregation tests (`CursorUsageClientTests.swift`).

mod common;

use chrono::{FixedOffset, Utc};
use common::{at, event, event_at};
use serde_json::json;
use tokenmon_core::ManualClock;
use tokenmon_provider_cursor::safe_int64;
use tokenmon_provider_cursor::{
    AVERAGE_WEEKS_PER_MONTH, CursorSnapshot, aggregate_cost_stats, aggregate_events, daily_tokens,
    date_from_epoch_number, estimated_weight_by_day, event_timestamp, hour_weights, parse_summary,
    parse_usage_events_page, quota_hour_weights, token_hour_weights,
};

fn utc() -> ManualClock {
    ManualClock::from_unix(0)
}

fn ms(t: chrono::DateTime<Utc>) -> String {
    t.timestamp_millis().to_string()
}

/// Cost is summed from the cycle start only.
#[test]
fn test_aggregate_cost_stats() {
    let now = at(2026, 8, 10, 12, 0);
    let events = [
        event_at(now, "default", 1202.0, 1_000_000, 500_000),
        event_at(
            now - chrono::Duration::days(2),
            "default",
            5000.0,
            2_000_000,
            0,
        ),
    ];
    let stats = aggregate_cost_stats(&events, now - chrono::Duration::days(20));
    assert!((stats.metered_cycle_usd - 62.02).abs() < 1e-9);
    assert_eq!(stats.cycle_tokens, 3_500_000);
    assert_eq!(stats.cycle_input_tokens, 3_000_000);
    assert_eq!(stats.cycle_output_tokens, 500_000);
    let recent = aggregate_cost_stats(&events, now - chrono::Duration::days(1));
    assert!((recent.metered_cycle_usd - 12.02).abs() < 1e-9);
}

/// `requestsCosts` weights win; token weights are the fallback.
#[test]
fn test_hour_weights_bucket_by_requests_costs() {
    let day = at(2026, 8, 10, 0, 0);
    let mk = |h, extra: serde_json::Value| {
        let mut v = json!({"timestamp": ms(at(2026, 8, 10, h, 0)), "model": "m"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        event(v)
    };
    let events = [
        mk(10, json!({"requestsCosts": 2.5})),
        mk(10, json!({"requestsCosts": 1.5})),
        mk(14, json!({"tokenUsage": {"inputTokens": 150}})),
    ];
    let w = hour_weights(&utc(), &events, day);
    assert_eq!(w[10], 4.0);
    assert_eq!(w[14], 150.0);
    assert_eq!(w[9], 0.0);
}

/// 100 + 300 cents on a $400 plan is the average-weeks-per-month constant.
#[test]
fn test_quota_hour_weights_use_charged_cents_and_plan_limit() {
    let day = at(2026, 8, 10, 0, 0);
    let events = [
        event_at(at(2026, 8, 10, 10, 5), "m", 100.0, 1, 0),
        event_at(at(2026, 8, 10, 10, 40), "m", 300.0, 1, 0),
    ];
    let w = quota_hour_weights(&utc(), &events, day, Some(400.0));
    assert!((w[10] - AVERAGE_WEEKS_PER_MONTH).abs() < 1e-9);
    assert!(
        quota_hour_weights(&utc(), &events, day, None)
            .iter()
            .all(|v| *v == 0.0)
    );
    assert!(
        quota_hour_weights(&utc(), &events, day, Some(0.0))
            .iter()
            .all(|v| *v == 0.0)
    );
}

/// All four token kinds are summed per hour.
#[test]
fn test_token_hour_weights_aggregate_event_tokens() {
    let e = event(json!({"timestamp": ms(at(2026, 8, 10, 9, 0)), "model": "m",
        "tokenUsage": {"inputTokens": 100, "outputTokens": 50, "cacheReadTokens": 25, "cacheWriteTokens": 5}}));
    assert_eq!(
        token_hour_weights(&utc(), &[e], at(2026, 8, 10, 0, 0))[9],
        180
    );
}

/// Hours are bucketed in the clock's local zone.
#[test]
fn test_hour_weights_use_the_local_offset() {
    let clock = ManualClock::from_unix(0).with_offset(FixedOffset::west_opt(8 * 3600).unwrap());
    let e = event_at(at(2026, 8, 10, 3, 0), "m", 0.0, 1, 0); // 19:00 local on Aug 9
    let day_start = common::at(2026, 8, 9, 8, 0); // local midnight Aug 9
    assert_eq!(token_hour_weights(&clock, &[e], day_start)[19], 1);
}

/// Total, raw page count and event count.
#[test]
fn test_parse_usage_events_page() {
    let t = ms(at(2026, 8, 10, 1, 0));
    let body = json!({"totalUsageEventsCount": 2, "usageEventsDisplay": [
        {"timestamp": t, "model": "default"}, {"timestamp": t, "model": "default"}]});
    let page = parse_usage_events_page(body.to_string().as_bytes()).unwrap();
    assert_eq!((page.total, page.page_count, page.events.len()), (2, 2, 2));
}

/// The total may arrive as a numeric string, and defaults to 0.
#[test]
fn test_parse_usage_events_page_coerces_total() {
    let page = parse_usage_events_page(br#"{"totalUsageEventsCount":"1234"}"#).unwrap();
    assert_eq!(page.total, 1234);
    assert_eq!(parse_usage_events_page(b"{}").unwrap().total, 0);
}

/// Skipped events (bot, no timestamp) still count toward the page size.
#[test]
fn test_parse_usage_events_page_counts_skipped_events() {
    let t = ms(at(2026, 8, 10, 1, 0));
    let body = json!({"usageEventsDisplay": [
        {"timestamp": t, "model": "default"},
        {"timestamp": t, "model": "grok-bot-default"},
        {"model": "default"}]});
    let page = parse_usage_events_page(body.to_string().as_bytes()).unwrap();
    assert_eq!((page.page_count, page.events.len()), (3, 1));
}

/// Non-object roots are typed errors.
#[test]
fn test_parse_usage_events_page_rejects_non_objects() {
    assert!(parse_usage_events_page(b"[1,2]").is_err());
    assert!(parse_usage_events_page(b"").is_err());
}

/// Conversions saturate instead of trapping.
#[test]
fn test_safe_integer_conversion_does_not_trap() {
    assert_eq!(safe_int64(1e30), i64::MAX);
    assert_eq!(safe_int64(-1e30), i64::MIN);
    assert_eq!(safe_int64(f64::NAN), 0);
    assert_eq!(safe_int64(f64::INFINITY), 0);
}

/// A huge token count saturates.
#[test]
fn test_token_count_handles_huge_values() {
    let e =
        event(json!({"timestamp": ms(at(2026, 8, 10, 1, 0)), "tokenUsage": {"inputTokens": 1e30}}));
    assert_eq!(e.input_tokens, i64::MAX);
    assert_eq!(e.total_tokens, i64::MAX);
}

/// String timestamps are milliseconds above 1e12 and seconds below it.
#[test]
fn test_event_timestamp_string_units() {
    let stamp = |s: &str| event_timestamp(json!({"timestamp": s}).as_object().unwrap());
    assert_eq!(stamp("1775418973898").unwrap().timestamp(), 1_775_418_973);
    assert_eq!(stamp("1775418973").unwrap().timestamp(), 1_775_418_973);
    assert!(stamp("1775418973898").is_some());
    assert_eq!(stamp("0"), None);
    assert_eq!(stamp("-5"), None);
    assert_eq!(stamp("soon"), None);
    assert_eq!(date_from_epoch_number(f64::NAN), None);
    assert_eq!(date_from_epoch_number(1e300), None);
}

/// Bot events are excluded; estimates clip to the cycle.
#[test]
fn test_daily_estimate_weight_by_day_excludes_bot_and_clips_to_cycle() {
    let mk = |d, h, model: &str, tokens: i64| {
        event(
            json!({"timestamp": ms(at(2026, 8, d, h, 0)), "model": model,
            "tokenUsage": {"inputTokens": tokens}}),
        )
    };
    let events: Vec<_> = [
        Some(mk(22, 10, "default", 5000)),
        Some(mk(22, 20, "default", 300)),
        None, // grok-bot is dropped by the parser
        Some(mk(23, 12, "default", 1220)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let bot = event_json_is_none(
        &json!({"timestamp": ms(at(2026, 8, 22, 21, 0)), "model": "grok-bot-default"}),
    );
    assert!(bot);
    let by_day = estimated_weight_by_day(
        &utc(),
        &events,
        at(2026, 8, 22, 17, 0),
        Some(at(2026, 8, 23, 17, 0)),
        at(2026, 8, 23, 13, 0),
    );
    let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 8, d).unwrap();
    assert_eq!(by_day.len(), 2);
    assert_eq!(by_day[&day(22)], 300.0);
    assert_eq!(by_day[&day(23)], 1220.0);
}

fn event_json_is_none(v: &serde_json::Value) -> bool {
    tokenmon_provider_cursor::CursorUsageEvent::from_json(v.as_object().expect("object")).is_none()
}

/// Grokbot traffic never reaches any aggregation.
#[test]
fn test_grok_bot_events_are_excluded_from_aggregations() {
    let now = at(2026, 8, 10, 12, 0);
    let t = ms(at(2026, 8, 10, 11, 0));
    let raw = json!({"totalUsageEventsCount": 2, "usageEventsDisplay": [
        {"timestamp": t, "model": "default", "chargedCents": 100, "tokenUsage": {"inputTokens": 1000}},
        {"timestamp": t, "model": "grok-bot-default", "chargedCents": 900, "tokenUsage": {"inputTokens": 9000}}]});
    let events = parse_usage_events_page(raw.to_string().as_bytes())
        .unwrap()
        .events;
    assert_eq!(events.len(), 1);
    let stats = aggregate_cost_stats(&events, now - chrono::Duration::days(1));
    assert_eq!((stats.metered_cycle_usd, stats.cycle_tokens), (1.0, 1000));
    assert_eq!(
        hour_weights(&utc(), &events, at(2026, 8, 10, 0, 0))[11],
        1000.0
    );
    assert_eq!(
        token_hour_weights(&utc(), &events, at(2026, 8, 10, 0, 0))[11],
        1000
    );
    let q = quota_hour_weights(&utc(), &events, at(2026, 8, 10, 0, 0), Some(400.0));
    assert!(q[11] < 2.0);
}

/// Cost falls back to `tokenUsage.totalCents`; per-day tokens split by model.
#[test]
fn test_daily_tokens_roll_up_per_day_and_model() {
    let e1 = event(json!({"timestamp": ms(at(2026, 8, 10, 1, 0)), "model": "a",
        "tokenUsage": {"inputTokens": 10, "outputTokens": 5, "totalCents": 150}}));
    let e2 = event(json!({"timestamp": ms(at(2026, 8, 10, 2, 0)), "model": "b",
        "tokenUsage": {"inputTokens": 4}}));
    let days = daily_tokens(&utc(), &[e1, e2]);
    let day = &days[&chrono::NaiveDate::from_ymd_opt(2026, 8, 10).unwrap()];
    assert_eq!(day.cost_usd, Some(1.5));
    assert_eq!(
        (day.input_tokens, day.output_tokens, day.total_tokens()),
        (14, 5, 19)
    );
    let tokens_of = |name: &str| day.models.iter().find(|m| m.name == name).map(|m| m.tokens);
    assert_eq!(tokens_of("a"), Some(15));
    assert_eq!(tokens_of("b"), Some(4));
}

/// The aggregate bundle ties the pieces together for one day.
#[test]
fn test_aggregate_events_bundles_cost_hourly_and_tokens() {
    let now = at(2026, 8, 10, 12, 0);
    let snap: CursorSnapshot =
        parse_summary(common::summary_body(now).to_string().as_bytes(), now).unwrap();
    let events = [event_at(at(2026, 8, 10, 11, 0), "m", 250.0, 10, 5)];
    let a = aggregate_events(&utc(), &events, &snap, now - chrono::Duration::days(5), now);
    assert_eq!(a.cost_stats.metered_cycle_usd, 2.5);
    assert!(!a.hourly.is_empty());
    assert_eq!(a.hourly.day_start, at(2026, 8, 10, 0, 0));
    assert_eq!(a.tokens_by_day.len(), 1);
}
