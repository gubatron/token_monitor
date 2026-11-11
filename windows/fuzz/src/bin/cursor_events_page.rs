//! Invariants: the events-page parser never panics; `page_count` covers every
//! parsed event; timestamps are not before the epoch; counts saturate; and aggregating the
//! parsed events (cost, hourly, per-day) yields finite, non-negative figures.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::ManualClock;
use tokenmon_provider_cursor::{
    aggregate_cost_stats, daily_tokens, estimated_weight_by_day, hour_weights,
    parse_usage_events_page, quota_hour_weights, token_hour_weights,
};

fuzz_target!(|data: &[u8]| {
    let Ok(page) = parse_usage_events_page(data) else {
        return;
    };
    assert!(page.events.len() <= page.page_count);
    for e in &page.events {
        // Not before the epoch: sub-second values such as 0.5 s are valid, and
        // a positive value under 1 ns ("770E-77") truncates to the epoch.
        assert!(e.date >= chrono::DateTime::UNIX_EPOCH);
        assert!(e.requests_costs.is_finite() && e.requests_costs >= 0.0);
        assert!(e.charged_cents.is_finite() && e.charged_cents >= 0.0);
        assert!(e.weight().is_finite() && e.weight() > 0.0);
        assert!(!e.model.starts_with("grok-bot"));
    }
    let clock = ManualClock::from_unix(1_786_000_000);
    let Some(first) = page.events.first() else {
        return;
    };
    let day = first.date;
    let stats = aggregate_cost_stats(&page.events, first.date);
    assert!(stats.metered_cycle_usd.is_finite() && stats.metered_cycle_usd >= 0.0);
    for w in hour_weights(&clock, &page.events, day)
        .into_iter()
        .chain(quota_hour_weights(&clock, &page.events, day, Some(40.0)))
    {
        assert!(w.is_finite() && w >= 0.0);
    }
    let _ = token_hour_weights(&clock, &page.events, day);
    for day in daily_tokens(&clock, &page.events).values() {
        assert!(day.cost_usd.is_none_or(f64::is_finite));
    }
    for w in estimated_weight_by_day(&clock, &page.events, first.date, None, first.date).values() {
        assert!(w.is_finite() && *w >= 0.0);
    }
});
