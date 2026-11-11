//! Conversions from the provider crates' snapshot types to the panel inputs.
//!
//! The snapshot alone does not carry everything a panel shows: the daily
//! chart (`daily` / `week`), the persisted expand flags and the poller/auth
//! state come from the app. `From` fills what the snapshot knows and leaves
//! the rest at its "absent" value; use the `with_*` builders to add them.

use std::collections::HashSet;

use tokenmon_core::Srgb;
use tokenmon_core::usage::Calendar;
use tokenmon_provider_chatgpt::{ChatGptSnapshot, UsageWindow};
use tokenmon_provider_claude::hover::ClaudeModelHoverDetail;
use tokenmon_provider_claude::{
    ClaudeCodeUsageSummary, ClaudeSnapshot, ClaudeUsageWindow, UsageTotals,
};
use tokenmon_provider_cursor::{CursorSnapshot, PoolKind};
use tokenmon_provider_grok::models::{
    CreditBalance, ProductCatalog, ProductUsage, WeeklyUsageSnapshot,
};
use tokenmon_provider_grokbot::GrokbotSnapshot;
use tokenmon_provider_opencode::snapshot::{
    ModelUsage as OpenCodeModel, OpenCodeSnapshot, WindowKind, WindowUsage,
};
use tokenmon_provider_openrouter::{
    BudgetSource as ProviderBudgetSource, ModelUsage as OpenRouterModel, OpenRouterSnapshot,
};

use super::claude_chatgpt::{
    ChatGptSnapshotInput, ClaudeCodeInput, ClaudeSnapshotInput, CodeModelInput, CodeTotalsInput,
    WindowInput,
};
use super::common::DailyBudgetInput;
use super::grok::{GrokSnapshotInput, ProductInput};
use super::grokbot_cursor::{
    CursorCostInput, CursorPoolInput, CursorSnapshotInput, GrokbotSnapshotInput,
};
use super::opencode_openrouter::{
    BudgetSource, ORANGE, OpenCodeLocalStatsInput, OpenCodeModelInput, OpenCodeSnapshotInput,
    OpenCodeWindowInput, OpenCodeWindowKind, OpenRouterModelInput, OpenRouterSnapshotInput,
};

fn tokens(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

// --------------------------------- Grok ------------------------------------

fn product(p: &ProductUsage) -> ProductInput {
    let (r, g, b) = p.color_token.srgb();
    ProductInput {
        short_name: ProductCatalog::short_name(&p.id),
        percent: p.percent_of_pool,
        color: Srgb::rgb(r, g, b),
    }
}

impl GrokSnapshotInput {
    /// Builds the panel input honouring the user's visible-product set.
    pub fn from_snapshot(
        s: &WeeklyUsageSnapshot,
        visible: &HashSet<String>,
        week: Option<DailyBudgetInput>,
    ) -> Self {
        Self {
            used_percent: s.used_percent,
            resets_at: s.resets_at,
            pool_products: ProductCatalog::panel_products(&s.products, visible)
                .iter()
                .map(product)
                .collect(),
            category_products: ProductCatalog::category_products(&s.products, visible)
                .iter()
                .map(product)
                .collect(),
            extra_credits_balance: s
                .extra_credits_balance
                .as_ref()
                .map_or(0.0, CreditBalance::value),
            week,
        }
    }
}

/// All known products visible, no daily chart.
impl From<&WeeklyUsageSnapshot> for GrokSnapshotInput {
    fn from(s: &WeeklyUsageSnapshot) -> Self {
        let all = ProductCatalog::normalize_visible_ids(
            &ProductCatalog::KNOWN_IDS
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
        );
        Self::from_snapshot(s, &all, None)
    }
}

// -------------------------------- Grokbot ----------------------------------

impl GrokbotSnapshotInput {
    /// Builds the panel input; `cal` decides the pool cadence from the span.
    pub fn from_snapshot(
        s: &GrokbotSnapshot,
        cal: &Calendar<'_>,
        daily: Option<DailyBudgetInput>,
    ) -> Self {
        Self {
            pool_name: s.usage_pool(cal).display_name().to_owned(),
            used_percent: s.used_percent,
            entitlement_caption: s.entitlement.caption_text(),
            has_included_allowance: s.has_included_allowance,
            resets_at: s.resets_at,
            daily,
        }
    }
}

// --------------------------------- Cursor ----------------------------------

impl From<&CursorSnapshot> for CursorSnapshotInput {
    fn from(s: &CursorSnapshot) -> Self {
        Self {
            plan_name: s.display_plan_name(),
            pools: s
                .pools
                .iter()
                .map(|p| CursorPoolInput {
                    label: pool_label(p.kind).to_owned(),
                    used_percent: p.used_percent,
                    resets_at: p.resets_at,
                })
                .collect(),
            resets_at: s.resets_at(),
            daily: None,
            cost: s.cost_stats.map(|c| CursorCostInput {
                metered_cycle_usd: c.metered_cycle_usd,
                cycle_tokens: tokens(c.cycle_tokens),
                cycle_input_tokens: tokens(c.cycle_input_tokens),
                cycle_output_tokens: tokens(c.cycle_output_tokens),
            }),
        }
    }
}

fn pool_label(kind: PoolKind) -> &'static str {
    kind.track_label()
}

// --------------------------------- Claude ----------------------------------

fn claude_window(w: &ClaudeUsageWindow) -> WindowInput {
    WindowInput {
        label: w.scope_name.clone(),
        used_percent: Some(w.used_percent),
        resets_at: w.resets_at,
    }
}

impl ClaudeSnapshotInput {
    /// Builds the panel input. `weekly_resets_at` is the poller's live or
    /// projected weekly reset; `None` falls back to the window's own.
    pub fn from_snapshot(
        s: &ClaudeSnapshot,
        weekly_resets_at: Option<chrono::DateTime<chrono::Utc>>,
        daily: Option<DailyBudgetInput>,
    ) -> Self {
        Self {
            pool_name: s.usage_pool().display_name().to_owned(),
            five_hour: s.five_hour.as_ref().map(claude_window),
            seven_day: s.seven_day.as_ref().map(claude_window),
            weekly_label: s.weekly_label(),
            weekly_resets_at: weekly_resets_at
                .or_else(|| s.seven_day.as_ref().and_then(|w| w.resets_at)),
            daily,
        }
    }
}

impl From<&ClaudeSnapshot> for ClaudeSnapshotInput {
    fn from(s: &ClaudeSnapshot) -> Self {
        Self::from_snapshot(s, None, None)
    }
}

fn totals(t: &UsageTotals) -> CodeTotalsInput {
    CodeTotalsInput {
        tokens: tokens(t.tokens),
        cost_usd: t.cost_usd,
        has_unpriced: t.has_unpriced,
    }
}

impl ClaudeCodeInput {
    /// Builds the Claude Code section; `models_expanded` is the persisted flag.
    pub fn from_summary(s: &ClaudeCodeUsageSummary, models_expanded: bool) -> Self {
        Self {
            month: totals(&s.month),
            all_time: totals(&s.all_time),
            month_models: s
                .month_models
                .iter()
                .map(|m| CodeModelInput {
                    name: m.display_name(),
                    is_unpriced: m.is_unpriced,
                    cost_usd: m.cost_usd,
                    share_percent: m.share_percent,
                    hover: Some(ClaudeModelHoverDetail::new(m, s)),
                })
                .collect(),
            models_expanded,
        }
    }
}

// --------------------------------- ChatGPT ---------------------------------

fn gpt_window(w: &UsageWindow, fallback: &str) -> WindowInput {
    WindowInput {
        label: Some(w.label(fallback)),
        used_percent: Some(w.used_percent),
        resets_at: w.resets_at,
    }
}

impl From<&ChatGptSnapshot> for ChatGptSnapshotInput {
    fn from(s: &ChatGptSnapshot) -> Self {
        Self {
            plan_name: s.display_plan_name(),
            pool_name: s.usage_pool().display_name().to_owned(),
            primary: s.primary.as_ref().map(|w| gpt_window(w, "5-Hour Window")),
            secondary: s.secondary.as_ref().map(|w| gpt_window(w, "Weekly")),
            limit_reached: s.limit_reached,
            allowed: s.allowed,
        }
    }
}

// --------------------------------- OpenCode --------------------------------

fn window_kind(k: WindowKind) -> OpenCodeWindowKind {
    match k {
        WindowKind::Rolling5h => OpenCodeWindowKind::FiveHour,
        WindowKind::Weekly => OpenCodeWindowKind::Weekly,
        WindowKind::Monthly => OpenCodeWindowKind::Monthly,
    }
}

fn window(w: &WindowUsage) -> OpenCodeWindowInput {
    OpenCodeWindowInput {
        kind: window_kind(w.kind),
        used_percent: w.used_percent(),
        resets_at: w.resets_at,
    }
}

fn opencode_model(m: &OpenCodeModel) -> OpenCodeModelInput {
    OpenCodeModelInput {
        name: tokenmon_provider_opencode::model_name::display_name(&m.model_id),
        cost_usd: m.cost_usd,
        is_cost_estimated: m.is_cost_estimated,
        is_cost_unpriced: m.is_cost_unpriced,
        percent_of_window: m.percent_of_window,
        color: tokenmon_provider_opencode::model_name::color_for_provider(
            &m.provider_id,
            &format!("{}/{}", m.provider_id, m.model_id),
            ORANGE,
        ),
    }
}

impl OpenCodeSnapshotInput {
    /// Builds the panel input. The caller supplies the persisted UI flags and
    /// the daily chart.
    pub fn from_snapshot(
        s: &OpenCodeSnapshot,
        show_all_models: bool,
        models_expanded: bool,
        daily: Option<DailyBudgetInput>,
    ) -> Self {
        Self {
            pool_name: s.usage_pool().display_name().to_owned(),
            windows: s.windows.iter().map(window).collect(),
            is_estimated: s.is_estimated,
            daily,
            models: s.models.iter().map(opencode_model).collect(),
            show_all_models,
            models_expanded,
            local_stats: s.has_local_stats().then(|| OpenCodeLocalStatsInput {
                monthly_usd: s.monthly_estimated_usd,
                tokens: tokens(s.monthly_tokens),
                input_tokens: tokens(s.monthly_input_tokens),
                output_tokens: tokens(s.monthly_output_tokens),
            }),
        }
    }
}

impl From<&OpenCodeSnapshot> for OpenCodeSnapshotInput {
    fn from(s: &OpenCodeSnapshot) -> Self {
        Self::from_snapshot(s, false, true, None)
    }
}

// -------------------------------- OpenRouter -------------------------------

fn or_model(m: &OpenRouterModel) -> OpenRouterModelInput {
    OpenRouterModelInput {
        model_id: m.model_id.clone(),
        is_revealed: m.model_id != m.activity_slug,
        cost_usd: m.cost_usd,
        is_cost_estimated: m.is_cost_estimated,
        percent_of_window: m.percent_of_window,
    }
}

impl OpenRouterSnapshotInput {
    /// Builds the panel input; `show_all_models` is the UI toggle.
    pub fn from_snapshot(s: &OpenRouterSnapshot, show_all_models: bool) -> Self {
        let used_percent = s
            .budget_usd
            .filter(|b| *b > 0.0)
            .map(|b| (s.used_usd / b * 100.0).clamp(0.0, 100.0));
        Self {
            budget_source: s.budget_source.map(|b| match b {
                ProviderBudgetSource::AccountCredits => BudgetSource::AccountCredits,
                ProviderBudgetSource::KeyLimit => BudgetSource::KeyLimit,
            }),
            budget_usd: s.budget_usd,
            used_percent,
            used_usd: s.used_usd,
            remaining_usd: s.remaining_usd,
            account_used_usd: s.account_used_usd,
            key_usage_usd: s.key_usage_usd,
            key_usage_daily_usd: s.key_usage_daily_usd,
            models: s.models.iter().map(or_model).collect(),
            show_all_models,
        }
    }
}

impl From<&OpenRouterSnapshot> for OpenRouterSnapshotInput {
    fn from(s: &OpenRouterSnapshot) -> Self {
        Self::from_snapshot(s, false)
    }
}
