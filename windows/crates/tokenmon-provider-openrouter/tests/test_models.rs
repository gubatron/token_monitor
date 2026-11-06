//! Port of OpenRouterModelsTests.swift (spec 03 section 7.8).
#![allow(clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use tokenmon_provider_openrouter::{
    BudgetSource, CostEstimator, CreditsData, KeyData, NoCostEstimator, OpenRouterSnapshot,
    parse_activity, parse_credits, parse_key,
};

fn key(usage: f64, limit: Option<f64>) -> KeyData {
    KeyData {
        label: Some("Test Key".into()),
        usage,
        usage_daily: Some(1.0),
        usage_weekly: Some(2.0),
        usage_monthly: Some(3.0),
        limit,
        limit_remaining: limit.map(|l| l - usage),
        limit_reset: None,
        is_free_tier: Some(false),
        is_management_key: Some(true),
    }
}

fn build(key: &KeyData, credits: Option<CreditsData>) -> OpenRouterSnapshot {
    let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    OpenRouterSnapshot::build(key, credits, None, now, &NoCostEstimator)
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-3, "{a} != {b}");
}

/// Verifies the `/key` payload decodes label, usage, daily, limit and flag.
#[test]
fn test_parse_key_payload() {
    let k = parse_key(
        br#"{"data":{"label":"TokenMon Key","usage":25.5,"usage_daily":2.5,"usage_weekly":10.0,
        "usage_monthly":20.0,"limit":100,"limit_remaining":74.5,"limit_reset":"monthly",
        "is_free_tier":false,"is_management_key":true}}"#,
    )
    .unwrap();
    assert_eq!(k.label.as_deref(), Some("TokenMon Key"));
    approx(k.usage, 25.5);
    approx(k.usage_daily.unwrap(), 2.5);
    approx(k.limit.unwrap(), 100.0);
    assert_eq!(k.is_management_key, Some(true));
}

/// Verifies the `/credits` payload.
#[test]
fn test_parse_credits_payload() {
    let c = parse_credits(br#"{"data":{"total_credits":100.5,"total_usage":25.75}}"#).unwrap();
    approx(c.total_credits, 100.5);
    approx(c.total_usage, 25.75);
}

/// Account credits are preferred over a key limit.
#[test]
fn test_budget_prefers_account_credits() {
    let s = build(
        &key(20.0, Some(50.0)),
        Some(CreditsData {
            total_credits: 120.0,
            total_usage: 30.0,
        }),
    );
    assert_eq!(s.budget_source, Some(BudgetSource::AccountCredits));
    approx(s.budget_usd.unwrap(), 120.0);
    approx(s.used_usd, 30.0);
    approx(s.remaining_usd.unwrap(), 90.0);
    approx(s.used_percent().unwrap(), 25.0);
    assert_eq!(s.budget_stat_title(), "Credits purchased");
}

/// An inference key without `/credits` falls back to its own limit.
#[test]
fn test_budget_falls_back_to_key_limit() {
    let mut k = key(25.0, Some(100.0));
    k.is_management_key = Some(false);
    let s = build(&k, None);
    assert_eq!(s.budget_source, Some(BudgetSource::KeyLimit));
    approx(s.used_usd, 25.0);
    approx(s.remaining_usd.unwrap(), 75.0);
    approx(s.used_percent().unwrap(), 25.0);
    assert_eq!(s.budget_stat_title(), "Key spending limit");
    assert_eq!(s.budget_source.unwrap().label(), "Key limit");
}

/// No limit and no credits: spend only, no percent.
#[test]
fn test_no_budget_yields_nil_percent() {
    let mut k = key(12.5, None);
    k.limit_remaining = None;
    let s = build(&k, None);
    assert!(s.budget_source.is_none() && s.budget_usd.is_none());
    assert!(s.remaining_usd.is_none() && s.used_percent().is_none());
    approx(s.used_usd, 12.5);
    assert_eq!(s.budget_stat_title(), "Credit limit");
}

/// With a reset window, `limit - limit_remaining` is the window spend.
#[test]
fn test_key_limit_window_uses_limit_remaining_when_reset_set() {
    let mut k = key(25.0, Some(100.0));
    k.limit_remaining = Some(90.0);
    k.limit_reset = Some("monthly".into());
    let s = build(&k, None);
    approx(s.used_usd, 10.0);
    approx(s.remaining_usd.unwrap(), 90.0);
    approx(s.used_percent().unwrap(), 10.0);
}

/// Without `limit_remaining`, the matching windowed usage field is used.
#[test]
fn test_key_limit_window_falls_back_to_matching_usage_field() {
    let mut k = key(25.0, Some(100.0));
    k.limit_remaining = None;
    k.usage_weekly = Some(14.0);
    k.limit_reset = Some("Weekly".into());
    let s = build(&k, None);
    approx(s.used_usd, 14.0);
    approx(s.remaining_usd.unwrap(), 86.0);
}

/// An unknown reset token with no signal keeps all-time usage.
#[test]
fn test_key_limit_unknown_reset_keeps_all_time_usage() {
    let mut k = key(25.0, Some(100.0));
    k.limit_remaining = None;
    k.usage_daily = None;
    k.usage_weekly = None;
    k.usage_monthly = None;
    k.limit_reset = Some("hourly".into());
    let s = build(&k, None);
    approx(s.used_usd, 25.0);
    approx(s.remaining_usd.unwrap(), 75.0);
}

/// A zero credit balance is not a denominator.
#[test]
fn test_zero_credit_balance_has_no_denominator() {
    let s = build(
        &key(5.0, None),
        Some(CreditsData {
            total_credits: 0.0,
            total_usage: 0.0,
        }),
    );
    assert!(s.budget_source.is_none() && s.used_percent().is_none());
}

/// Overdrawn accounts clamp used to zero.
#[test]
fn test_negative_usage_clamps_to_zero() {
    let s = build(
        &key(10.0, Some(8.0)),
        Some(CreditsData {
            total_credits: 8.0,
            total_usage: -0.5,
        }),
    );
    approx(s.used_usd, 0.0);
    approx(s.used_percent().unwrap(), 0.0);
}

/// Minimal `/key` bodies default flags to false and spends to zero.
#[test]
fn test_key_parsing_defaults() {
    let k = parse_key(br#"{"data":{"usage":3}}"#).unwrap();
    assert!(k.label.is_none() && k.limit.is_none());
    let s = build(&k, None);
    assert!(!s.is_management_key && !s.is_free_tier);
    approx(s.key_usage_daily_usd, 0.0);
}

/// Rows decode, including the stealth row's large token counts.
#[test]
fn test_parse_activity_payload() {
    let rows = parse_activity(
        br#"{"data":[
        {"date":"2026-09-20","model":"z-ai/glm-5.3-flash","provider_name":"Z.ai","usage":2.5,
         "requests":10,"prompt_tokens":1000,"completion_tokens":200,"reasoning_tokens":0},
        {"date":"2026-09-20","model":"stealth/ox-alpha","provider_name":"Stealth","usage":0,
         "requests":284,"prompt_tokens":3489882,"completion_tokens":110726,"reasoning_tokens":0}]}"#,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].model, "stealth/ox-alpha");
    assert_eq!(rows[1].prompt_tokens, 3_489_882);
    assert_eq!(rows[1].completion_tokens, 110_726);
}

struct Glm;
impl CostEstimator for Glm {
    fn estimate(&self, bare: &str, prompt: i64, completion: i64) -> Option<f64> {
        (bare == "glm-5.3-flash")
            .then(|| prompt as f64 / 1e6 * 0.045 + completion as f64 / 1e6 * 0.14)
    }
}

fn row(model: &str, usage: f64, requests: i64, p: i64, c: i64) -> String {
    format!(
        r#"{{"date":"d","model":"{model}","usage":{usage},"requests":{requests},
        "prompt_tokens":{p},"completion_tokens":{c},"reasoning_tokens":0}}"#
    )
}

fn models(
    rows: &[String],
    est: &dyn CostEstimator,
) -> Vec<tokenmon_provider_openrouter::ModelUsage> {
    let body = format!(r#"{{"data":[{}]}}"#, rows.join(","));
    let rows = parse_activity(body.as_bytes()).unwrap();
    tokenmon_provider_openrouter::models_from_rows(&rows, est)
}

/// A revealed stealth model is renamed and valued at the public rate.
#[test]
fn test_stealth_model_revealed_and_valued() {
    let m = models(
        &[row("stealth/ox-alpha", 0.0, 284, 1_000_000, 1_000_000)],
        &Glm,
    );
    assert_eq!(m[0].model_id, "z-ai/glm-5.3-flash");
    assert_eq!(m[0].activity_slug, "stealth/ox-alpha");
    assert!(m[0].is_revealed() && m[0].is_cost_estimated);
    approx(m[0].cost_usd, 0.185);
}

/// Reported cost is kept as is and rows of one model aggregate.
#[test]
fn test_reported_cost_is_not_estimated_and_aggregates() {
    let m = models(
        &[
            row("z-ai/glm-5.3-flash", 2.1292, 498, 7_926_964, 71_931),
            row("z-ai/glm-5.3-flash", 0.5, 2, 10, 5),
        ],
        &Glm,
    );
    assert_eq!(m.len(), 1);
    approx(m[0].cost_usd, 2.6292);
    assert!(!m[0].is_cost_estimated);
    assert_eq!(m[0].requests, 500);
    approx(m[0].percent_of_window, 100.0);
}

/// A `:free` slug with no price stays at zero.
#[test]
fn test_unpriced_free_slug_stays_zero() {
    let m = models(
        &[row("moonshotai/kimi-k2.6:free", 0.0, 3, 39_644, 71)],
        &Glm,
    );
    approx(m[0].cost_usd, 0.0);
    assert!(!m[0].is_cost_estimated);
}

/// Cost then token ordering; shares add up to 100.
#[test]
fn test_models_sorted_by_cost_then_tokens() {
    let m = models(
        &[
            row("a/low", 1.0, 1, 1, 1),
            row("b/high", 3.0, 1, 1, 1),
            row("c/free", 0.0, 1, 10, 10),
            row("d/free", 0.0, 1, 50, 50),
        ],
        &NoCostEstimator,
    );
    let ids: Vec<_> = m.iter().map(|x| x.model_id.as_str()).collect();
    assert_eq!(ids, ["b/high", "a/low", "d/free", "c/free"]);
    approx(m.iter().map(|x| x.percent_of_window).sum(), 100.0);
}

/// Integral floats are accepted for counts; fractional ones are rejected.
#[test]
fn test_activity_counts_accept_integral_floats_only() {
    let ok = r#"{"data":[{"date":"d","model":"m","usage":1,"requests":2.0,"prompt_tokens":1e3,"completion_tokens":0,"reasoning_tokens":0}]}"#;
    assert_eq!(
        parse_activity(ok.as_bytes()).unwrap()[0].prompt_tokens,
        1000
    );
    let bad = ok.replace("2.0", "2.5");
    assert!(parse_activity(bad.as_bytes()).is_err());
}

/// Huge values never produce NaN or out-of-range percentages.
#[test]
fn test_extreme_numbers_keep_invariants() {
    let k =
        parse_key(br#"{"data":{"usage":1e308,"limit":1e-308,"limit_remaining":-1e308}}"#).unwrap();
    let s = build(
        &k,
        Some(CreditsData {
            total_credits: 1e308,
            total_usage: -1e308,
        }),
    );
    assert!(s.used_usd.is_finite() && s.used_usd >= 0.0);
    let p = s.used_percent().unwrap();
    assert!((0.0..=100.0).contains(&p));
}
