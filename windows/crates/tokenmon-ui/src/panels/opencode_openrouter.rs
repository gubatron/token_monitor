//! OpenCode (`OpenCodePanelView`, spec 03 section 3.12) and OpenRouter
//! (`OpenRouterPanelView`, spec 03 section 4.6) panels.

use chrono::{DateTime, Utc};
use tokenmon_core::{ProviderId, Srgb};

use super::common::{
    DailyBudgetInput, DailyBudgetVm, FooterVm, HeaderVm, MetricVm, ModelRowVm, ModelsCardVm,
    PanelContext, PanelKind, PanelVm, ProviderStatus, SignInAction, StaleVm, TrackVm,
    build_daily_budget, standard_frame, waiting_message,
};
use super::format;

/// `ModelPalette.orange` (OpenCode Go, 5-hour bar).
pub const ORANGE: Srgb = Srgb::rgb(1.0, 0.55, 0.0);
/// `ModelPalette.purple` (weekly/monthly bars, Zen).
pub const PURPLE: Srgb = Srgb::rgb(0.58, 0.44, 0.86);

// -------------------------------- OpenCode ---------------------------------

/// OpenCode limit window kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeWindowKind {
    /// Rolling 5 hours.
    FiveHour,
    /// Weekly.
    Weekly,
    /// Monthly.
    Monthly,
}

impl OpenCodeWindowKind {
    /// `5-Hour`, `Weekly`, `Monthly`.
    pub fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5-Hour",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
        }
    }
}

/// One OpenCode window.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeWindowInput {
    /// Window kind.
    pub kind: OpenCodeWindowKind,
    /// Used percent.
    pub used_percent: f64,
    /// Reset instant.
    pub resets_at: Option<DateTime<Utc>>,
}

/// One OpenCode model row.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeModelInput {
    /// `OpenCodeModelName.display`.
    pub name: String,
    /// Cost.
    pub cost_usd: f64,
    /// Cost estimated.
    pub is_cost_estimated: bool,
    /// No price known.
    pub is_cost_unpriced: bool,
    /// Share of window, percent.
    pub percent_of_window: f64,
    /// `ModelPalette.sRGB(forProvider:seed:)`.
    pub color: Srgb,
}

/// Local stats grid input.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeLocalStatsInput {
    /// Monthly estimated spend.
    pub monthly_usd: f64,
    /// Total tokens.
    pub tokens: u64,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
}

/// OpenCode snapshot input.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeSnapshotInput {
    /// `Usage Pool X`.
    pub pool_name: String,
    /// Windows in display order.
    pub windows: Vec<OpenCodeWindowInput>,
    /// Estimated from local sessions.
    pub is_estimated: bool,
    /// Daily bars (monthly).
    pub daily: Option<DailyBudgetInput>,
    /// Models by spend.
    pub models: Vec<OpenCodeModelInput>,
    /// Models list expanded (`View all`).
    pub show_all_models: bool,
    /// Persisted `openCodeModels` expanded state.
    pub models_expanded: bool,
    /// Local stats (when `hasLocalStats`).
    pub local_stats: Option<OpenCodeLocalStatsInput>,
}

/// OpenCode panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeInput {
    /// Auth and poller state.
    pub status: ProviderStatus,
    /// Snapshot.
    pub snapshot: Option<OpenCodeSnapshotInput>,
}

/// OpenCode usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeUsage {
    /// `Usage Pool X`.
    pub pool_title: String,
    /// One bar per window.
    pub tracks: Vec<TrackVm>,
    /// Orange estimate note.
    pub estimate_note: Option<&'static str>,
    /// Daily bars.
    pub daily: Option<DailyBudgetVm>,
    /// Top models card.
    pub models: Option<ModelsCardVm>,
    /// 2x2 local stats.
    pub metrics: Option<Vec<MetricVm>>,
    /// Plain (non-stale) error caption.
    pub error_text: Option<String>,
    /// Sign-in button when the session needs it.
    pub sign_in: Option<SignInAction>,
    /// Footer (sign-out only).
    pub footer: FooterVm,
}

fn window_track(w: &OpenCodeWindowInput, ctx: &PanelContext) -> TrackVm {
    let color = if w.kind == OpenCodeWindowKind::FiveHour {
        ORANGE
    } else {
        PURPLE
    };
    let caption = match (w.kind, w.resets_at) {
        (OpenCodeWindowKind::FiveHour, Some(t)) => Some(format::reset_caption_relative(t, ctx.now)),
        (OpenCodeWindowKind::FiveHour, None) if w.used_percent <= 0.0 => {
            Some("No usage in last 5h".to_owned())
        }
        (OpenCodeWindowKind::Weekly, Some(t)) => Some(format::reset_caption_short(t, ctx.tz)),
        (OpenCodeWindowKind::Monthly, Some(t)) => Some(ctx.reset_caption(t)),
        (_, None) => None,
    };
    TrackVm::new(w.kind.label(), w.used_percent, color, caption)
}

fn model_cost_text(m: &OpenCodeModelInput) -> String {
    if m.is_cost_unpriced && m.cost_usd == 0.0 {
        "No price".to_owned()
    } else if m.is_cost_estimated || m.is_cost_unpriced {
        format!("~{}", format::usd(m.cost_usd))
    } else {
        format::usd(m.cost_usd)
    }
}

/// Builds the OpenCode panel.
pub fn build_opencode(input: &OpenCodeInput, ctx: &PanelContext) -> PanelVm<OpenCodeUsage> {
    let header = HeaderVm::new(ProviderId::OpenCode, "OpenCode Go");
    let s = &input.status;
    let header = match standard_frame(
        header,
        s,
        input.snapshot.is_some(),
        "Sign in to the OpenCode console to load official rolling, weekly, and monthly usage.",
        SignInAction::to_provider(ProviderId::OpenCode),
    ) {
        Ok(frame) => return frame,
        Err(h) => h,
    };
    let Some(snap) = &input.snapshot else {
        unreachable!("standard_frame handles the no-snapshot case")
    };
    let rows: Vec<ModelRowVm> = snap
        .models
        .iter()
        .map(|m| ModelRowVm {
            name: m.name.clone(),
            cost_text: model_cost_text(m),
            tag: None,
            share_fraction: format::clamp_percent(m.percent_of_window) / 100.0,
            color: m.color,
        })
        .collect();
    PanelVm::with_usage(
        header,
        OpenCodeUsage {
            pool_title: format!("Usage Pool {}", snap.pool_name),
            tracks: snap.windows.iter().map(|w| window_track(w, ctx)).collect(),
            estimate_note: snap
                .is_estimated
                .then_some("Estimated from local sessions — sign in for console accuracy."),
            daily: snap.daily.as_ref().and_then(|d| build_daily_budget(d, ctx)),
            models: (!rows.is_empty()).then(|| {
                ModelsCardVm::new(
                    "Top Models by Spend",
                    true,
                    snap.models_expanded,
                    rows,
                    snap.show_all_models,
                )
            }),
            metrics: snap.local_stats.as_ref().map(|l| {
                vec![
                    MetricVm::new("Monthly spend", format::usd(l.monthly_usd)),
                    MetricVm::new("Total tokens", format::tokens(l.tokens)),
                    MetricVm::new("Input tokens", format::tokens(l.input_tokens)),
                    MetricVm::new("Output tokens", format::tokens(l.output_tokens)),
                ]
            }),
            error_text: s.last_error.clone(),
            sign_in: s
                .needs_sign_in
                .then(|| SignInAction::to_provider(ProviderId::OpenCode)),
            footer: FooterVm {
                stale: None,
                sign_in_again: None,
                sign_out: Some(super::common::SignOutVm::new(ProviderId::OpenCode)),
            },
        },
    )
}

// ------------------------------- OpenRouter --------------------------------

/// Where the budget comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetSource {
    /// Purchased account credits.
    AccountCredits,
    /// Per-key spending limit.
    KeyLimit,
}

impl BudgetSource {
    /// `Account credits` / `Key limit`.
    pub fn label(self) -> &'static str {
        match self {
            Self::AccountCredits => "Account credits",
            Self::KeyLimit => "Key limit",
        }
    }

    fn stat_title(self) -> &'static str {
        match self {
            Self::AccountCredits => "Credits purchased",
            Self::KeyLimit => "Key spending limit",
        }
    }
}

/// OpenRouter model row input.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterModelInput {
    /// Canonical slug.
    pub model_id: String,
    /// Real model hidden behind an alias.
    pub is_revealed: bool,
    /// Cost.
    pub cost_usd: f64,
    /// Cost is estimated.
    pub is_cost_estimated: bool,
    /// Share of window, percent.
    pub percent_of_window: f64,
}

/// OpenRouter snapshot input.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterSnapshotInput {
    /// Budget source, if any.
    pub budget_source: Option<BudgetSource>,
    /// Budget in USD.
    pub budget_usd: Option<f64>,
    /// Used percent (budget present).
    pub used_percent: Option<f64>,
    /// Used USD for the budget window.
    pub used_usd: f64,
    /// Remaining USD.
    pub remaining_usd: Option<f64>,
    /// `accountUsedUSD`.
    pub account_used_usd: Option<f64>,
    /// `keyUsageUSD`.
    pub key_usage_usd: f64,
    /// `keyUsageDailyUSD`.
    pub key_usage_daily_usd: f64,
    /// Models.
    pub models: Vec<OpenRouterModelInput>,
    /// `View all` toggled.
    pub show_all_models: bool,
}

/// OpenRouter panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterInput {
    /// Auth and poller state (`has_auth_error` unused).
    pub status: ProviderStatus,
    /// Snapshot.
    pub snapshot: Option<OpenRouterSnapshotInput>,
    /// Text currently typed in the key field.
    pub key_draft: String,
    /// User chose `Replace API Key…`.
    pub is_replacing: bool,
    /// `auth.lastAuthError`.
    pub auth_error: Option<String>,
}

/// API key entry row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntryVm {
    /// Field placeholder.
    pub placeholder: &'static str,
    /// `Add Key` or `Save Key`.
    pub button_label: &'static str,
    /// Button enabled (trimmed draft non-empty).
    pub button_enabled: bool,
    /// Red error below the field.
    pub error: Option<String>,
}

/// OpenRouter usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterUsage {
    /// `Credit Budget`.
    pub budget_title: &'static str,
    /// `N% Used` when a percent exists.
    pub used_text: Option<String>,
    /// Track (label hidden) when a budget exists.
    pub track: Option<TrackVm>,
    /// Text when the key has no limit.
    pub no_limit_text: Option<&'static str>,
    /// `Stats` grid.
    pub stats: Vec<MetricVm>,
    /// Models card.
    pub models: Option<ModelsCardVm>,
    /// Stale caption.
    pub stale: Option<StaleVm>,
    /// `Replace API Key…` link shown.
    pub replace_link: bool,
}

/// Whole OpenRouter panel.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterPanelVm {
    /// Frame and body.
    pub panel: PanelVm<OpenRouterUsage>,
    /// Key entry row, when visible.
    pub key_entry: Option<KeyEntryVm>,
    /// Sign-out control (usage state only).
    pub sign_out: Option<super::common::SignOutVm>,
}

/// Builds the OpenRouter panel.
pub fn build_openrouter(input: &OpenRouterInput, ctx: &PanelContext) -> OpenRouterPanelVm {
    let header = HeaderVm::new(ProviderId::OpenRouter, "OpenRouter");
    let s = &input.status;
    let entry = KeyEntryVm {
        placeholder: "sk-or-v1-…",
        button_label: if input.is_replacing {
            "Save Key"
        } else {
            "Add Key"
        },
        button_enabled: !input.key_draft.trim().is_empty(),
        error: input.auth_error.clone(),
    };
    let Some(snap) = &input.snapshot else {
        let (kind, text) = if s.needs_sign_in {
            (
                PanelKind::SignedOut,
                "Paste an OpenRouter API key to track spending against your purchased credits."
                    .to_owned(),
            )
        } else {
            (PanelKind::Waiting, waiting_message(s, "No usage data yet."))
        };
        return OpenRouterPanelVm {
            panel: PanelVm::frame(header, kind, Some(text), None),
            key_entry: Some(entry),
            sign_out: None,
        };
    };
    let accent = ProviderId::OpenRouter.panel_accent();
    let track = snap.budget_usd.zip(snap.used_percent).map(|(budget, pct)| {
        let caption = format!(
            "{} of {} · {} left",
            format::usd(snap.used_usd),
            format::usd(budget),
            format::usd(snap.remaining_usd.unwrap_or(0.0))
        );
        TrackVm::new("Credits used", pct, accent, Some(caption)).without_label()
    });
    let title = snap
        .budget_source
        .map_or("Credit limit", BudgetSource::stat_title);
    let stats = vec![
        MetricVm::new(
            title,
            snap.budget_usd
                .map_or_else(|| "None".to_owned(), format::usd),
        ),
        MetricVm::new(
            "Total spent",
            format::usd(snap.account_used_usd.unwrap_or(snap.key_usage_usd)),
        ),
        MetricVm::new("Remaining", format::usd(snap.remaining_usd.unwrap_or(0.0))),
        MetricVm::new("Spent today", format::usd(snap.key_usage_daily_usd)),
    ];
    let rows: Vec<ModelRowVm> = snap
        .models
        .iter()
        .map(|m| ModelRowVm {
            name: m.model_id.clone(),
            cost_text: format!(
                "{}{}",
                if m.is_cost_estimated { "~" } else { "" },
                format::usd(m.cost_usd)
            ),
            tag: m.is_revealed.then(|| "revealed".to_owned()),
            share_fraction: format::clamp_percent(m.percent_of_window) / 100.0,
            color: accent,
        })
        .collect();
    let show_entry = input.is_replacing || s.needs_sign_in;
    let meta = snap
        .budget_source
        .map_or("No credit limit", BudgetSource::label);
    OpenRouterPanelVm {
        panel: PanelVm::with_usage(
            header.with_meta(Some(meta.to_owned())),
            OpenRouterUsage {
                budget_title: "Credit Budget",
                used_text: snap.used_percent.map(format::percent_used),
                no_limit_text: track
                    .is_none()
                    .then_some("This key has no credit limit, so usage shows as spend only."),
                track,
                stats,
                models: (!rows.is_empty()).then(|| {
                    ModelsCardVm::new(
                        "Top Models by Spend",
                        false,
                        true,
                        rows,
                        snap.show_all_models,
                    )
                }),
                stale: StaleVm::from_status(s, ctx),
                replace_link: !show_entry,
            },
        ),
        key_entry: show_entry.then_some(entry),
        sign_out: Some(super::common::SignOutVm::new(ProviderId::OpenRouter)),
    }
}
