//! Small behaviours of the Cursor provider that the larger suites do not pin:
//! event weights, timestamp shapes, plan names, pool labels, the production
//! clients and the even-split fallback of the daily budget.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use common::{at, event};
use serde_json::json;
use tokenmon_core::usage::WeekOffset;
use tokenmon_core::{ManualClock, ProviderErrorContext};
use tokenmon_provider_cursor::{
    CursorHttp, CursorUsageClient, DailyBudgetInputs, PoolKind, build_daily_budget_days,
    display_plan_name, event_timestamp,
};

/// The weight falls back from request costs to tokens to charged cents to 1.
#[test]
fn test_event_weight_falls_back_through_charged_cents_to_one() {
    let stamp = at(2026, 8, 28, 10, 0).timestamp_millis().to_string();
    let cents_only = event(json!({"timestamp": stamp, "chargedCents": 7.5}));
    assert_eq!(cents_only.weight(), 7.5);
    let nothing = event(json!({"timestamp": stamp}));
    assert_eq!(nothing.weight(), 1.0);
    let costs = event(json!({"timestamp": stamp, "requestsCosts": 3.0, "chargedCents": 9.0}));
    assert_eq!(costs.weight(), 3.0);
    let tokens = event(json!({
        "timestamp": stamp,
        "chargedCents": 9.0,
        "tokenUsage": {"inputTokens": 40, "outputTokens": 2},
    }));
    assert_eq!(tokens.weight(), 42.0);
}

/// A timestamp that is neither a string nor a number is rejected.
#[test]
fn test_event_timestamp_rejects_other_json_types() {
    for value in [json!(true), json!(null), json!([1]), json!({"a": 1})] {
        let object = json!({ "timestamp": value });
        assert_eq!(event_timestamp(object.as_object().unwrap()), None);
    }
    assert_eq!(
        event_timestamp(json!({"model": "x"}).as_object().unwrap()),
        None,
        "a missing timestamp is rejected"
    );
}

/// Pool labels are stable and short.
#[test]
fn test_pool_kind_short_labels() {
    assert_eq!(PoolKind::Total.short_label(), "Total");
    assert_eq!(PoolKind::Auto.short_label(), "Auto");
    assert_eq!(PoolKind::Api.short_label(), "API");
}

/// Plan names: blank is plain Cursor, a Cursor-prefixed name is not doubled.
#[test]
fn test_display_plan_name_edge_cases() {
    assert_eq!(display_plan_name(None), "Cursor");
    assert_eq!(display_plan_name(Some("   ")), "Cursor");
    assert_eq!(display_plan_name(Some("pro")), "Cursor Pro");
    assert_eq!(display_plan_name(Some("cursor pro")), "Cursor pro");
}

/// The production constructors build without touching the network and report
/// the provider wording they were given.
#[test]
fn test_production_clients_construct_offline() {
    let clock = Arc::new(ManualClock::from_unix(0));
    let http = CursorHttp::production(ProviderErrorContext::Cursor, Some("1.0"), clock.clone())
        .expect("tls backend");
    assert_eq!(http.context(), ProviderErrorContext::Cursor);
    let client = CursorUsageClient::production(Some("1.0"), clock).expect("client");
    assert!(format!("{client:?}").contains("CursorUsageClient"));
}

/// Untracked usage is split evenly over estimated days that carry no weight.
#[test]
fn test_untracked_usage_splits_evenly_over_zero_weight_days() {
    let now = at(2026, 8, 28, 12, 0);
    let weights = BTreeMap::from([
        (at(2026, 8, 25, 0, 0).date_naive(), 0.0),
        (at(2026, 8, 26, 0, 0).date_naive(), 0.0),
    ]);
    let observed: HashMap<_, _> = HashMap::new();
    let days = build_daily_budget_days(
        &ManualClock::new(now),
        DailyBudgetInputs {
            observed_by_day: &observed,
            interrupted_window_start: None,
            estimated_weight_by_day: &weights,
            used_percent: 14.0,
            cycle_start: Some(at(2026, 8, 22, 17, 0)),
            cycle_end: Some(at(2026, 9, 22, 17, 0)),
            week_offset: WeekOffset::CURRENT,
            now,
        },
    )
    .unwrap();
    let spent = |d: u32| {
        days.iter()
            .find(|day| day.date == at(2026, 8, d, 0, 0))
            .unwrap()
            .spent_usd
    };
    assert!((spent(25) - 7.0).abs() < 0.001, "{}", spent(25));
    assert!((spent(26) - 7.0).abs() < 0.001, "{}", spent(26));
    assert_eq!(spent(27), 0.0);
}
