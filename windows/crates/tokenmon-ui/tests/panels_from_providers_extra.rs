//! Edge cases of the provider-snapshot to panel-input conversions: the
//! visible-product filter, Claude Code summaries, OpenCode models and
//! local stats, OpenRouter budget percentages and Cursor cost stats.
#![allow(missing_docs, clippy::unwrap_used)]

use std::collections::{BTreeMap, HashSet};

use chrono::{TimeZone, Utc};
use tokenmon_provider_claude::{ClaudeCodeUsageSummary, ModelUsageRow, UsageTotals};
use tokenmon_provider_cursor::{CursorCostStats, CursorPoolUsage, CursorSnapshot, PoolKind};
use tokenmon_provider_grok::models::{CreditBalance, ProductUsage, WeeklyUsageSnapshot};
use tokenmon_provider_opencode::snapshot::{ModelUsage as OcModel, OpenCodeSnapshot};
use tokenmon_provider_openrouter::{
    BudgetSource as ProviderBudget, ModelUsage as OrModel, OpenRouterSnapshot,
};
use tokenmon_ui::panels::claude_chatgpt::ClaudeCodeInput;
use tokenmon_ui::panels::grok::GrokSnapshotInput;
use tokenmon_ui::panels::grokbot_cursor::CursorSnapshotInput;
use tokenmon_ui::panels::opencode_openrouter::{
    BudgetSource, OpenCodeSnapshotInput, OpenRouterSnapshotInput,
};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
}

/// Only visible products appear, and the credit balance is carried over.
#[test]
fn test_grok_from_snapshot_honours_visible_set_and_credits() {
    let snap = WeeklyUsageSnapshot::new(now(), 40.0)
        .with_products(vec![
            ProductUsage::new("chat", "Chat", 30.0),
            ProductUsage::new("build", "Build", 10.0),
        ])
        .with_credits(CreditBalance::from_f64(12.5));
    let visible: HashSet<String> = ["chat".to_owned()].into();
    let input = GrokSnapshotInput::from_snapshot(&snap, &visible, None);
    assert_eq!(input.pool_products.len(), 1);
    assert_eq!(input.pool_products[0].percent, 30.0);
    assert_eq!(input.category_products.len(), 1);
    assert_eq!(input.extra_credits_balance, 12.5);
}

/// An empty visible set hides every product.
#[test]
fn test_grok_from_snapshot_with_empty_visible_set_shows_nothing() {
    let snap = WeeklyUsageSnapshot::new(now(), 10.0)
        .with_products(vec![ProductUsage::new("chat", "Chat", 10.0)]);
    let input = GrokSnapshotInput::from_snapshot(&snap, &HashSet::new(), None);
    assert_eq!(input.pool_products.len(), 0);
    assert_eq!(input.category_products.len(), 0);
}

/// Claude Code totals convert i64 tokens; negatives become 0.
#[test]
fn test_claude_code_input_maps_totals_models_and_flag() {
    let summary = ClaudeCodeUsageSummary {
        month: UsageTotals {
            tokens: 1_500,
            cost_usd: 3.25,
            has_unpriced: true,
        },
        all_time: UsageTotals {
            tokens: -7,
            cost_usd: 10.0,
            has_unpriced: false,
        },
        month_models: vec![ModelUsageRow {
            model_id: "claude-opus-4-5".into(),
            tokens: 1_500,
            cost_usd: 3.25,
            is_unpriced: false,
            share_percent: 100.0,
        }],
    };
    let input = ClaudeCodeInput::from_summary(&summary, true);
    assert_eq!(input.month.tokens, 1_500);
    assert!(input.month.has_unpriced);
    assert_eq!(input.all_time.tokens, 0, "negative tokens clamp to zero");
    assert_eq!(input.month_models.len(), 1);
    assert_eq!(
        input.month_models[0].name,
        summary.month_models[0].display_name()
    );
    assert_eq!(input.month_models[0].share_percent, 100.0);
    assert!(input.models_expanded);
    assert!(!ClaudeCodeInput::from_summary(&summary, false).models_expanded);
}

/// OpenCode models and local stats carry over with the caller's UI flags.
#[test]
fn test_opencode_from_snapshot_maps_models_and_local_stats() {
    let snap = OpenCodeSnapshot {
        models: vec![OcModel {
            provider_id: "opencode-go".into(),
            model_id: "kimi-k2".into(),
            cost_usd: 1.5,
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            session_count: 1,
            percent_of_window: 100.0,
            is_cost_estimated: true,
            is_cost_unpriced: false,
        }],
        is_estimated: true,
        monthly_tokens: 900,
        monthly_estimated_usd: 4.5,
        monthly_input_tokens: 600,
        monthly_output_tokens: -1,
        ..OpenCodeSnapshot::default()
    };
    let input = OpenCodeSnapshotInput::from_snapshot(&snap, true, false, None);
    assert!(input.show_all_models);
    assert!(!input.models_expanded);
    assert!(input.is_estimated);
    assert_eq!(input.models.len(), 1);
    assert_eq!(input.models[0].cost_usd, 1.5);
    assert!(input.models[0].is_cost_estimated);
    let stats = input.local_stats.unwrap();
    assert_eq!(stats.tokens, 900);
    assert_eq!(stats.input_tokens, 600);
    assert_eq!(stats.output_tokens, 0, "negative tokens clamp to zero");
}

fn or_snapshot(budget: Option<f64>, used: f64) -> OpenRouterSnapshot {
    let model = |id: &str, slug: &str, cost: f64, est: bool, pct: f64| OrModel {
        model_id: id.into(),
        activity_slug: slug.into(),
        requests: 1,
        prompt_tokens: 1,
        completion_tokens: 1,
        cost_usd: cost,
        is_cost_estimated: est,
        percent_of_window: pct,
    };
    OpenRouterSnapshot {
        key_label: None,
        is_management_key: false,
        is_free_tier: false,
        account_credits_usd: None,
        account_used_usd: Some(2.0),
        key_usage_usd: 1.0,
        key_usage_daily_usd: 0.5,
        key_usage_weekly_usd: 0.0,
        key_usage_monthly_usd: 0.0,
        key_limit_usd: budget,
        key_limit_remaining_usd: None,
        limit_reset: None,
        budget_source: Some(ProviderBudget::KeyLimit),
        budget_usd: budget,
        used_usd: used,
        remaining_usd: budget.map(|b| b - used),
        models: vec![
            model("openai/gpt-5", "openai/gpt-5", 0.4, false, 80.0),
            model("vendor/model", "vendor/model-20260101", 0.1, true, 20.0),
        ],
        fetched_at: now(),
    }
}

/// Used percent derives from the budget, models flag revealed slugs.
#[test]
fn test_openrouter_from_snapshot_computes_percent_and_reveals() {
    let input = OpenRouterSnapshotInput::from_snapshot(&or_snapshot(Some(4.0), 1.0), true);
    assert_eq!(input.used_percent, Some(25.0));
    assert_eq!(input.budget_source, Some(BudgetSource::KeyLimit));
    assert!(input.show_all_models);
    assert!(!input.models[0].is_revealed);
    assert!(input.models[1].is_revealed);
    assert!(input.models[1].is_cost_estimated);
}

/// Overspend clamps at 100; a zero or absent budget gives no percent.
#[test]
fn test_openrouter_from_snapshot_clamps_and_handles_missing_budget() {
    let over = OpenRouterSnapshotInput::from(&or_snapshot(Some(1.0), 5.0));
    assert_eq!(over.used_percent, Some(100.0));
    assert!(!over.show_all_models);
    let zero = OpenRouterSnapshotInput::from(&or_snapshot(Some(0.0), 1.0));
    assert_eq!(zero.used_percent, None);
    let none = OpenRouterSnapshotInput::from(&or_snapshot(None, 1.0));
    assert_eq!(none.used_percent, None);
}

/// Cursor cost stats become the cost input, clamping negative token counts.
#[test]
fn test_cursor_snapshot_maps_cost_stats() {
    let snap = CursorSnapshot {
        fetched_at: now(),
        used_percent: 10.0,
        pools: vec![CursorPoolUsage {
            kind: PoolKind::Total,
            used_percent: 10.0,
            resets_at: None,
        }],
        billing_cycle_start: None,
        billing_cycle_end: None,
        membership_type: None,
        plan_used_usd: None,
        plan_limit_usd: None,
        on_demand_enabled: false,
        on_demand_used_usd: None,
        on_demand_limit_usd: None,
        cost_stats: Some(CursorCostStats {
            metered_cycle_usd: 2.5,
            cycle_tokens: 100,
            cycle_input_tokens: 60,
            cycle_output_tokens: -5,
        }),
        account_email: None,
        daily_tokens: BTreeMap::default(),
    };
    let cost = CursorSnapshotInput::from(&snap).cost.unwrap();
    assert_eq!(cost.metered_cycle_usd, 2.5);
    assert_eq!(cost.cycle_input_tokens, 60);
    assert_eq!(cost.cycle_output_tokens, 0);
}
