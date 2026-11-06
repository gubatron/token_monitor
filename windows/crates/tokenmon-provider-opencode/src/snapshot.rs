//! Snapshot value types shared by the console client and the local scan
//! (spec 03 sections 3.2 and 3.6).

use chrono::{DateTime, Utc};
use tokenmon_core::usage::UsagePool;

/// Which usage window a limit bar describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// Rolling five-hour window.
    Rolling5h,
    /// Calendar week (UTC Monday start).
    Weekly,
    /// Subscription month.
    Monthly,
}

impl WindowKind {
    /// Go plan default limit in USD.
    pub const fn default_limit_usd(self) -> f64 {
        match self {
            Self::Rolling5h => 12.0,
            Self::Weekly => 30.0,
            Self::Monthly => 60.0,
        }
    }

    /// The cadence this window contributes to the panel's usage pool.
    pub const fn usage_pool(self) -> UsagePool {
        match self {
            Self::Rolling5h => UsagePool::Hourly,
            Self::Weekly => UsagePool::Weekly,
            Self::Monthly => UsagePool::Monthly,
        }
    }

    /// Bar label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rolling5h => "5-Hour",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
        }
    }
}

/// One usage window: spend against a dollar limit.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowUsage {
    /// Which window.
    pub kind: WindowKind,
    /// Dollars used.
    pub used_usd: f64,
    /// Dollar limit.
    pub limit_usd: f64,
    /// When the window resets, if known.
    pub resets_at: Option<DateTime<Utc>>,
    /// Distinct sessions that contributed.
    pub session_count: u32,
}

impl WindowUsage {
    /// `used / limit * 100` clamped to `0..=100`; 0 when the limit is not
    /// positive or the arithmetic is not finite.
    pub fn used_percent(&self) -> f64 {
        if self.limit_usd > 0.0 {
            let p = self.used_usd / self.limit_usd * 100.0;
            if p.is_finite() {
                p.clamp(0.0, 100.0)
            } else {
                0.0
            }
        } else {
            0.0
        }
    }
}

/// Per-model usage over the weekly window.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsage {
    /// Provider id (`opencode-go` or `opencode`).
    pub provider_id: String,
    /// Model id.
    pub model_id: String,
    /// Sum of billable cost.
    pub cost_usd: f64,
    /// Input tokens.
    pub input_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Cache-read tokens.
    pub cache_read_tokens: i64,
    /// Cache-write tokens.
    pub cache_write_tokens: i64,
    /// Parity quirk count, see spec 03 section 3.6.
    pub session_count: u32,
    /// Share of the window's total cost, 0..=100.
    pub percent_of_window: f64,
    /// Any row's cost came from the price table.
    pub is_cost_estimated: bool,
    /// Any row had tokens but no price.
    pub is_cost_unpriced: bool,
}

/// The OpenCode snapshot shown in the panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenCodeSnapshot {
    /// Limit windows, in `5h, weekly, monthly` order, missing ones dropped.
    pub windows: Vec<WindowUsage>,
    /// Weekly per-model usage.
    pub models: Vec<ModelUsage>,
    /// `true` when computed from the local database.
    pub is_estimated: bool,
    /// Monthly total tokens.
    pub monthly_tokens: i64,
    /// Monthly estimated spend.
    pub monthly_estimated_usd: f64,
    /// Monthly input tokens.
    pub monthly_input_tokens: i64,
    /// Monthly output tokens.
    pub monthly_output_tokens: i64,
}

impl OpenCodeSnapshot {
    /// Stacked 5-hour + weekly + monthly limits: one cadence keeps its name,
    /// several (or none) give `Mixed`.
    pub fn usage_pool(&self) -> UsagePool {
        let pools: Vec<UsagePool> = self.windows.iter().map(|w| w.kind.usage_pool()).collect();
        UsagePool::combining(&pools)
    }

    /// The window of `kind`, if present.
    pub fn window(&self, kind: WindowKind) -> Option<&WindowUsage> {
        self.windows.iter().find(|w| w.kind == kind)
    }

    /// Headline percent: monthly, else weekly, else 0. The spec's
    /// `monthly.usedPercent ?? weekly.usedPercent ?? 0`.
    pub fn monthly_used_percent(&self) -> f64 {
        self.window(WindowKind::Monthly)
            .or_else(|| self.window(WindowKind::Weekly))
            .map_or(0.0, WindowUsage::used_percent)
    }

    /// Whether local statistics contributed anything to display.
    pub fn has_local_stats(&self) -> bool {
        !self.models.is_empty() || self.monthly_tokens > 0 || self.monthly_estimated_usd > 0.0
    }

    /// Keeps this (console) snapshot's windows but takes the local models and
    /// monthly totals when present (`mergeLocalModels`).
    #[must_use]
    pub fn merged_with_local(self, local: Option<&Self>) -> Self {
        let Some(local) = local else { return self };
        let take_totals = local.monthly_tokens > 0 || local.monthly_estimated_usd > 0.0;
        Self {
            models: if local.models.is_empty() {
                self.models
            } else {
                local.models.clone()
            },
            monthly_tokens: if take_totals {
                local.monthly_tokens
            } else {
                self.monthly_tokens
            },
            monthly_estimated_usd: if take_totals {
                local.monthly_estimated_usd
            } else {
                self.monthly_estimated_usd
            },
            monthly_input_tokens: if take_totals {
                local.monthly_input_tokens
            } else {
                self.monthly_input_tokens
            },
            monthly_output_tokens: if take_totals {
                local.monthly_output_tokens
            } else {
                self.monthly_output_tokens
            },
            ..self
        }
    }
}
