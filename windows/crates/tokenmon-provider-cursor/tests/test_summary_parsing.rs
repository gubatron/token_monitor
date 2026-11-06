//! Port of the `parseSummary` tests of `CursorUsageClientTests.swift`.

mod common;

use common::at;
use serde_json::json;
use tokenmon_provider_cursor::{PoolKind, parse_email, parse_summary};

/// Total, Auto and API percents, dates, plan money and the plan name.
#[test]
fn test_parse_summary_uses_total_auto_api_percents() {
    let body = json!({
        "billingCycleStart": "2026-07-13T00:00:00.000Z",
        "billingCycleEnd": "2026-08-14T12:00:00.000Z",
        "membershipType": "ultra",
        "individualUsage": {
            "plan": {"used": 7600, "limit": 40000, "totalPercentUsed": 19,
                     "autoPercentUsed": 16, "apiPercentUsed": 30},
            "onDemand": {"enabled": true, "used": 2309, "limit": null}
        }
    });
    let snap = parse_summary(body.to_string().as_bytes(), at(2026, 8, 1, 0, 0)).unwrap();
    assert_eq!(snap.used_percent, 19.0);
    assert_eq!(snap.pools.len(), 3);
    let remaining: Vec<f64> = snap
        .pools
        .iter()
        .map(tokenmon_provider_cursor::CursorPoolUsage::remaining_percent)
        .collect();
    assert_eq!(remaining, [81.0, 84.0, 70.0]);
    assert_eq!(snap.pools[1].kind, PoolKind::Auto);
    assert_eq!(snap.plan_used_usd, Some(76.0));
    assert_eq!(snap.plan_limit_usd, Some(400.0));
    assert_eq!(snap.display_plan_name(), "Cursor Ultra");
    assert_eq!(snap.billing_cycle_start, Some(at(2026, 7, 13, 0, 0)));
    assert_eq!(snap.billing_cycle_end, Some(at(2026, 8, 14, 12, 0)));
    assert_eq!(snap.resets_at(), snap.billing_cycle_end);
    assert!(snap.on_demand_enabled);
    assert_eq!(snap.on_demand_used_usd, Some(23.09));
    assert_eq!(snap.on_demand_limit_usd, None);
}

/// Omitted cycle dates stay `None`; nothing is invented.
#[test]
fn test_parse_summary_leaves_cycle_dates_nil_when_omitted() {
    let snap = parse_summary(
        br#"{"individualUsage":{"plan":{"totalPercentUsed":3}}}"#,
        at(2026, 8, 1, 0, 0),
    )
    .unwrap();
    assert_eq!(snap.billing_cycle_start, None);
    assert_eq!(snap.billing_cycle_end, None);
}

/// Auto 10 and API 30 average to 20 when the total is missing.
#[test]
fn test_parse_summary_averages_auto_and_api_when_total_missing() {
    let body = br#"{"individualUsage":{"plan":{"autoPercentUsed":10,"apiPercentUsed":30}}}"#;
    let snap = parse_summary(body, at(2026, 8, 1, 0, 0)).unwrap();
    assert_eq!(snap.used_percent, 20.0);
    assert_eq!(snap.pools.len(), 3);
}

/// With no percents at all, used over limit cents is the total.
#[test]
fn test_parse_summary_falls_back_to_used_over_limit_cents() {
    let body = br#"{"individualUsage":{"plan":{"used":2500,"limit":10000}}}"#;
    let snap = parse_summary(body, at(2026, 8, 1, 0, 0)).unwrap();
    assert_eq!(snap.used_percent, 25.0);
    assert_eq!(
        (snap.plan_used_usd, snap.plan_limit_usd),
        (Some(25.0), Some(100.0))
    );
    assert_eq!(snap.pools.len(), 1);
}

/// Numeric strings and `{"val":..}` wrappers are read; wrong types are ignored.
#[test]
fn test_parse_summary_tolerates_string_numbers_and_wrong_types() {
    let body = json!({"individualUsage": {"plan": {"totalPercentUsed": {"val": "250"},
        "autoPercentUsed": [1], "used": true}}, "billingCycleEnd": 5});
    let snap = parse_summary(body.to_string().as_bytes(), at(2026, 8, 1, 0, 0)).unwrap();
    assert_eq!(snap.used_percent, 100.0);
    assert_eq!(snap.billing_cycle_end, None);
}

/// Non-object and invalid roots are bad responses, not panics.
#[test]
fn test_parse_summary_rejects_non_objects() {
    assert!(parse_summary(b"[]", at(2026, 8, 1, 0, 0)).is_err());
    assert!(parse_summary(b"{\"individualUsage\":", at(2026, 8, 1, 0, 0)).is_err());
}

/// Pool labels follow the spec table.
#[test]
fn test_pool_labels() {
    assert_eq!(PoolKind::Total.track_label(), "Total");
    assert_eq!(PoolKind::Auto.track_label(), "Auto + Composer");
    assert_eq!(PoolKind::Api.track_label(), "API (Other Models)");
    assert_eq!(PoolKind::Api.short_label(), "API");
}

/// `parseEmail` only returns strings containing `@`.
#[test]
fn test_parse_email_requires_at_sign() {
    assert_eq!(
        parse_email(br#"{"email":"me@x.io"}"#).as_deref(),
        Some("me@x.io")
    );
    assert_eq!(parse_email(br#"{"email":"me"}"#), None);
    assert_eq!(parse_email(br#"{"email":5}"#), None);
    assert_eq!(parse_email(b"not json"), None);
}
