//! Cursor snapshot types (spec 02 section 3.3).

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};

use crate::json::clamp_percent;

/// Which Cursor quota a pool tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PoolKind {
    /// The headline total.
    Total,
    /// Auto + Composer.
    Auto,
    /// API (other models).
    Api,
}

impl PoolKind {
    /// Label on the usage track.
    pub const fn track_label(self) -> &'static str {
        match self {
            Self::Total => "Total",
            Self::Auto => "Auto + Composer",
            Self::Api => "API (Other Models)",
        }
    }

    /// Short label for compact UI.
    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Total => "Total",
            Self::Auto => "Auto",
            Self::Api => "API",
        }
    }
}

/// One pool's usage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorPoolUsage {
    /// Pool kind.
    pub kind: PoolKind,
    /// Used percent, clamped to 0..=100.
    pub used_percent: f64,
    /// When the pool resets.
    pub resets_at: Option<DateTime<Utc>>,
}

impl CursorPoolUsage {
    /// `clamp(100 - used)`.
    pub fn remaining_percent(&self) -> f64 {
        clamp_percent(100.0 - self.used_percent)
    }
}

/// Billing-cycle figures derived from usage events.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CursorCostStats {
    /// Metered spend this cycle in USD.
    pub metered_cycle_usd: f64,
    /// Total tokens in the cycle.
    pub cycle_tokens: i64,
    /// Input tokens in the cycle.
    pub cycle_input_tokens: i64,
    /// Output tokens in the cycle.
    pub cycle_output_tokens: i64,
}

/// Tokens and cost of one local day, as shown in the daily-bar hover label.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DayTokenUsage {
    /// Input tokens.
    pub input: i64,
    /// Output tokens.
    pub output: i64,
    /// Cache-read tokens.
    pub cache_read: i64,
    /// Cache-write tokens.
    pub cache_write: i64,
    /// Charged cost in USD.
    pub cost_usd: f64,
    /// Total tokens per model name.
    pub by_model: BTreeMap<String, i64>,
}

impl DayTokenUsage {
    /// Sum of the four token kinds (saturating).
    pub fn total(&self) -> i64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

/// What one successful Cursor refresh publishes.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorSnapshot {
    /// When the data was fetched.
    pub fetched_at: DateTime<Utc>,
    /// The total pool's used percent.
    pub used_percent: f64,
    /// Total first, then Auto and API when present.
    pub pools: Vec<CursorPoolUsage>,
    /// Billing cycle start.
    pub billing_cycle_start: Option<DateTime<Utc>>,
    /// Billing cycle end (the reset).
    pub billing_cycle_end: Option<DateTime<Utc>>,
    /// Raw membership type, for example `ultra`.
    pub membership_type: Option<String>,
    /// Plan spend in USD.
    pub plan_used_usd: Option<f64>,
    /// Plan limit in USD.
    pub plan_limit_usd: Option<f64>,
    /// Whether on-demand billing is enabled.
    pub on_demand_enabled: bool,
    /// On-demand spend in USD.
    pub on_demand_used_usd: Option<f64>,
    /// On-demand limit in USD.
    pub on_demand_limit_usd: Option<f64>,
    /// Event-derived cycle figures; `None` when events were unavailable.
    pub cost_stats: Option<CursorCostStats>,
    /// Account email from `/api/auth/me`.
    pub account_email: Option<String>,
    /// Tokens per local day.
    pub daily_tokens: BTreeMap<NaiveDate, DayTokenUsage>,
}

impl CursorSnapshot {
    /// The headline reset: the billing cycle end.
    pub fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.billing_cycle_end
    }

    /// `Cursor`, `Cursor Ultra`, `Cursor pro`, ... from the membership type.
    pub fn display_plan_name(&self) -> String {
        display_plan_name(self.membership_type.as_deref())
    }
}

/// `displayPlanName`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::display_plan_name;
/// assert_eq!(display_plan_name(None), "Cursor");
/// assert_eq!(display_plan_name(Some("ultra")), "Cursor Ultra");
/// assert_eq!(display_plan_name(Some("PRO")), "Cursor Pro");
/// assert_eq!(display_plan_name(Some("cursor pro")), "Cursor pro");
/// ```
pub fn display_plan_name(membership: Option<&str>) -> String {
    let trimmed = membership.map(str::trim).unwrap_or_default();
    if trimmed.is_empty() {
        return "Cursor".to_owned();
    }
    let capitalized = capitalize_first(trimmed);
    if trimmed.to_lowercase().starts_with("cursor") {
        capitalized
    } else {
        format!("Cursor {capitalized}")
    }
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}
