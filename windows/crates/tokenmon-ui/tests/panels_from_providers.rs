//! `From` impls from provider snapshot types to the panel inputs.
#![allow(missing_docs, clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use tokenmon_core::usage::Calendar;
use tokenmon_provider_chatgpt::{ChatGptSnapshot, UsageResponse, UsageWindow};
use tokenmon_provider_claude::{ClaudeSnapshot, ClaudeUsageWindow};
use tokenmon_provider_cursor::{CursorPoolUsage, CursorSnapshot, PoolKind};
use tokenmon_provider_grok::models::{ProductUsage, WeeklyUsageSnapshot};
use tokenmon_provider_grokbot::{GrokbotEntitlement, GrokbotSnapshot};
use tokenmon_provider_opencode::snapshot::{OpenCodeSnapshot, WindowKind, WindowUsage};
use tokenmon_ui::panels::claude_chatgpt::{ChatGptSnapshotInput, ClaudeSnapshotInput};
use tokenmon_ui::panels::grok::GrokSnapshotInput;
use tokenmon_ui::panels::grokbot_cursor::{CursorSnapshotInput, GrokbotSnapshotInput};
use tokenmon_ui::panels::opencode_openrouter::{OpenCodeSnapshotInput, OpenCodeWindowKind};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap()
}

/// Grok products map to short names, colors and the chip list.
#[test]
fn test_grok_snapshot_maps_products_and_credits() {
    let snap = WeeklyUsageSnapshot::new(now(), 36.0).with_products(vec![
        ProductUsage::new("chat", "Chat", 23.0),
        ProductUsage::new("build", "Build", 13.0),
    ]);
    let input = GrokSnapshotInput::from(&snap);
    assert_eq!(input.used_percent, 36.0);
    assert_eq!(input.pool_products.len(), 2);
    assert_eq!(input.pool_products[0].percent, 23.0);
    assert!(input.category_products.len() >= 2);
    assert_eq!(input.extra_credits_balance, 0.0);
    assert!(input.week.is_none());
}

/// Cursor plan name, pool labels and cost stats carry over.
#[test]
fn test_cursor_snapshot_maps_plan_and_pools() {
    let snap = CursorSnapshot {
        fetched_at: now(),
        used_percent: 55.0,
        pools: vec![CursorPoolUsage {
            kind: PoolKind::Total,
            used_percent: 55.0,
            resets_at: None,
        }],
        billing_cycle_start: None,
        billing_cycle_end: Some(now()),
        membership_type: Some("ultra".into()),
        plan_used_usd: None,
        plan_limit_usd: None,
        on_demand_enabled: false,
        on_demand_used_usd: None,
        on_demand_limit_usd: None,
        cost_stats: None,
        account_email: None,
        daily_tokens: std::collections::BTreeMap::default(),
    };
    let input = CursorSnapshotInput::from(&snap);
    assert_eq!(input.plan_name, "Cursor Ultra");
    assert_eq!(input.pools[0].label, "Total");
    assert_eq!(input.resets_at, Some(now()));
    assert!(input.cost.is_none());
}

/// Grokbot uses the calendar to pick the pool and the entitlement caption.
#[test]
fn test_grokbot_snapshot_maps_pool_and_entitlement() {
    let snap = GrokbotSnapshot {
        fetched_at: now(),
        used_percent: 42.0,
        period_start: None,
        resets_at: Some(now()),
        entitlement: GrokbotEntitlement::Cursor,
        account_email: None,
        has_included_allowance: true,
    };
    let input = GrokbotSnapshotInput::from_snapshot(&snap, &Calendar::utc(), None);
    assert_eq!(input.pool_name, "Weekly");
    assert_eq!(input.entitlement_caption, "via Cursor");
}

/// Claude windows, weekly label and pool name carry over.
#[test]
fn test_claude_snapshot_maps_windows_and_labels() {
    let snap = ClaudeSnapshot {
        fetched_at: now(),
        five_hour: Some(ClaudeUsageWindow {
            used_percent: 12.0,
            resets_at: None,
            scope_name: None,
        }),
        seven_day: Some(ClaudeUsageWindow {
            used_percent: 60.0,
            resets_at: Some(now()),
            scope_name: Some("Opus".into()),
        }),
        account_email: None,
    };
    let input = ClaudeSnapshotInput::from(&snap);
    assert_eq!(input.pool_name, "Mixed");
    assert_eq!(input.weekly_label, "Weekly · Opus");
    assert_eq!(input.weekly_resets_at, Some(now()));
    assert_eq!(input.five_hour.unwrap().used_percent, Some(12.0));
}

/// ChatGPT plan casing and window labels come from the provider crate.
#[test]
fn test_chatgpt_snapshot_maps_plan_and_windows() {
    let window = |secs| UsageWindow {
        used_percent: 20.0,
        resets_at: None,
        window_seconds: secs,
    };
    let snap = ChatGptSnapshot::new(
        UsageResponse {
            plan_name: Some("plus".into()),
            allowed: true,
            limit_reached: false,
            primary: Some(window(Some(18_000))),
            secondary: Some(window(Some(604_800))),
        },
        now(),
    );
    let input = ChatGptSnapshotInput::from(&snap);
    assert_eq!(input.plan_name, "Plus");
    assert_eq!(input.pool_name, "Mixed");
    assert_eq!(
        input.primary.unwrap().label.as_deref(),
        Some("5-Hour Window")
    );
    assert_eq!(input.secondary.unwrap().label.as_deref(), Some("Weekly"));
}

/// OpenCode window kinds map one to one; empty local stats are omitted.
#[test]
fn test_opencode_snapshot_maps_windows() {
    let snap = OpenCodeSnapshot {
        windows: vec![WindowUsage {
            kind: WindowKind::Rolling5h,
            used_usd: 6.0,
            limit_usd: 12.0,
            resets_at: None,
            session_count: 1,
        }],
        ..OpenCodeSnapshot::default()
    };
    let input = OpenCodeSnapshotInput::from(&snap);
    assert_eq!(input.windows[0].kind, OpenCodeWindowKind::FiveHour);
    assert_eq!(input.windows[0].used_percent, 50.0);
    assert!(input.local_stats.is_none());
}

/// The OpenCode pool title follows the stacked windows instead of a fixed
/// "Mixed" (issue #57): one window keeps its cadence, several read Mixed.
#[test]
fn test_opencode_pool_name_follows_windows() {
    let window = |kind| WindowUsage {
        kind,
        used_usd: 0.0,
        limit_usd: 12.0,
        resets_at: None,
        session_count: 0,
    };
    let pool = |kinds: &[WindowKind]| {
        let snap = OpenCodeSnapshot {
            windows: kinds.iter().copied().map(window).collect(),
            ..OpenCodeSnapshot::default()
        };
        OpenCodeSnapshotInput::from(&snap).pool_name
    };
    assert_eq!(pool(&[WindowKind::Monthly]), "Monthly");
    assert_eq!(pool(&[WindowKind::Rolling5h]), "Hourly");
    assert_eq!(pool(&[WindowKind::Weekly, WindowKind::Monthly]), "Mixed");
}

/// Issue #61: model rows show the short display name and the provider palette
/// colour (Go orange, Zen purple, seeded for the rest).
#[test]
fn test_opencode_model_rows_use_display_name_and_palette() {
    use tokenmon_provider_opencode::snapshot::ModelUsage;
    let model = |provider: &str, id: &str| ModelUsage {
        provider_id: provider.to_owned(),
        model_id: id.to_owned(),
        cost_usd: 1.0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        session_count: 1,
        percent_of_window: 50.0,
        is_cost_estimated: false,
        is_cost_unpriced: false,
    };
    let snap = OpenCodeSnapshot {
        models: vec![
            model("opencode-go", "gpt-5.4"),
            model("opencode", "glm-5.1"),
            model("OpenCode-Go", "kimi-k2.5"),
            model("xai", "grok-4"),
        ],
        ..OpenCodeSnapshot::default()
    };
    let m = OpenCodeSnapshotInput::from(&snap).models;
    assert_eq!(m[0].name, "GPT-5.4");
    assert_eq!(m[0].color, tokenmon_ui::panels::opencode_openrouter::ORANGE);
    assert_eq!(m[1].color, tokenmon_ui::panels::opencode_openrouter::PURPLE);
    assert_eq!(m[2].color, m[0].color);
    assert_eq!(
        m[3].color,
        tokenmon_provider_opencode::model_name::color_for_provider(
            "xai",
            "xai/grok-4",
            tokenmon_ui::panels::opencode_openrouter::ORANGE
        )
    );
}
