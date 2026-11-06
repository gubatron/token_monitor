//! [`OpenRouterSnapshot`]: budget selection, reset window and derived values.

use chrono::{DateTime, Utc};

use crate::activity::{CostEstimator, ModelUsage, models_from_rows};
use crate::models::{ActivityRow, CreditsData, KeyData};

/// Where the budget denominator comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetSource {
    /// Purchased account credits (`/credits`, management keys).
    AccountCredits,
    /// The per-key spending limit from `/key`.
    KeyLimit,
}

impl BudgetSource {
    /// Panel meta label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::AccountCredits => "Account credits",
            Self::KeyLimit => "Key limit",
        }
    }
}

/// The window a per-key limit resets on (`limit_reset`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitReset {
    /// `daily`.
    Daily,
    /// `weekly`.
    Weekly,
    /// `monthly`.
    Monthly,
    /// Any other token; only `limit_remaining` can describe the window.
    Other,
}

impl LimitReset {
    /// Case-insensitive parse; never fails (unknown is [`Self::Other`]).
    pub fn parse(raw: &str) -> Self {
        match raw.to_lowercase().as_str() {
            "daily" => Self::Daily,
            "weekly" => Self::Weekly,
            "monthly" => Self::Monthly,
            _ => Self::Other,
        }
    }
}

/// Everything the panel shows for OpenRouter.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRouterSnapshot {
    /// Key label.
    pub key_label: Option<String>,
    /// Management-key flag (default false).
    pub is_management_key: bool,
    /// Free-tier flag (default false).
    pub is_free_tier: bool,
    /// Purchased credits, when `/credits` loaded.
    pub account_credits_usd: Option<f64>,
    /// Account spend, when `/credits` loaded.
    pub account_used_usd: Option<f64>,
    /// All-time spend of this key.
    pub key_usage_usd: f64,
    /// Spend today.
    pub key_usage_daily_usd: f64,
    /// Spend this week.
    pub key_usage_weekly_usd: f64,
    /// Spend this month.
    pub key_usage_monthly_usd: f64,
    /// Per-key limit.
    pub key_limit_usd: Option<f64>,
    /// Per-key limit remaining.
    pub key_limit_remaining_usd: Option<f64>,
    /// Reset window of the key limit.
    pub limit_reset: Option<LimitReset>,
    /// Chosen budget source.
    pub budget_source: Option<BudgetSource>,
    /// Budget denominator.
    pub budget_usd: Option<f64>,
    /// Spend measured against the budget (never negative).
    pub used_usd: f64,
    /// Budget left.
    pub remaining_usd: Option<f64>,
    /// Per-model spend, highest first.
    pub models: Vec<ModelUsage>,
    /// When the data was fetched.
    pub fetched_at: DateTime<Utc>,
}

/// Maps NaN and overflowed values to finite ones so no NaN or infinity ever
/// reaches the UI.
pub(crate) fn finite(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(f64::MIN, f64::MAX)
    }
}

fn windowed_usage(key: &KeyData, reset: &str) -> Option<f64> {
    match LimitReset::parse(reset) {
        LimitReset::Daily => key.usage_daily,
        LimitReset::Weekly => key.usage_weekly,
        LimitReset::Monthly => key.usage_monthly,
        LimitReset::Other => None,
    }
}

struct Budget {
    source: BudgetSource,
    total: f64,
    used: f64,
    remaining: f64,
}

fn credits_budget(credits: Option<CreditsData>) -> Option<Budget> {
    let c = credits.filter(|c| c.total_credits > 0.0)?;
    Some(Budget {
        source: BudgetSource::AccountCredits,
        total: c.total_credits,
        used: c.total_usage,
        remaining: finite(c.total_credits - c.total_usage).max(0.0),
    })
}

fn key_limit_budget(key: &KeyData) -> Option<Budget> {
    let limit = key.limit.filter(|l| *l > 0.0)?;
    let window_usage = key.limit_reset.as_deref().and_then(|reset| {
        key.limit_remaining
            .map(|r| finite(limit - r).max(0.0))
            .or_else(|| windowed_usage(key, reset))
    });
    let remaining = key
        .limit_remaining
        .or_else(|| window_usage.map(|w| finite(limit - w).max(0.0)))
        .unwrap_or_else(|| finite(limit - key.usage).max(0.0));
    Some(Budget {
        source: BudgetSource::KeyLimit,
        total: limit,
        used: window_usage.unwrap_or(key.usage),
        remaining,
    })
}

impl OpenRouterSnapshot {
    /// Builds the snapshot (`OpenRouterSnapshot.build`).
    ///
    /// Account credits win when positive, then a positive per-key limit
    /// (measured over its reset window), else spend only. `used_usd` is
    /// clamped to be non-negative and finite.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::Utc;
    /// use tokenmon_provider_openrouter::{
    ///     BudgetSource, CreditsData, NoCostEstimator, OpenRouterSnapshot, parse_key,
    /// };
    ///
    /// let key = parse_key(br#"{"data":{"usage":20,"limit":50}}"#).unwrap();
    /// let credits = CreditsData { total_credits: 120.0, total_usage: 30.0 };
    /// let s = OpenRouterSnapshot::build(&key, Some(credits), None, Utc::now(), &NoCostEstimator);
    /// assert_eq!(s.budget_source, Some(BudgetSource::AccountCredits));
    /// assert_eq!(s.used_percent(), Some(25.0));
    /// ```
    pub fn build(
        key: &KeyData,
        credits: Option<CreditsData>,
        activity: Option<&[ActivityRow]>,
        fetched_at: DateTime<Utc>,
        estimator: &dyn CostEstimator,
    ) -> Self {
        let budget = credits_budget(credits).or_else(|| key_limit_budget(key));
        let used = budget.as_ref().map_or(key.usage, |b| b.used);
        Self {
            key_label: key.label.clone(),
            is_management_key: key.is_management_key.unwrap_or(false),
            is_free_tier: key.is_free_tier.unwrap_or(false),
            account_credits_usd: credits.map(|c| c.total_credits),
            account_used_usd: credits.map(|c| c.total_usage),
            key_usage_usd: key.usage,
            key_usage_daily_usd: key.usage_daily.unwrap_or(0.0),
            key_usage_weekly_usd: key.usage_weekly.unwrap_or(0.0),
            key_usage_monthly_usd: key.usage_monthly.unwrap_or(0.0),
            key_limit_usd: key.limit,
            key_limit_remaining_usd: key.limit_remaining,
            limit_reset: key.limit_reset.as_deref().map(LimitReset::parse),
            budget_source: budget.as_ref().map(|b| b.source),
            budget_usd: budget.as_ref().map(|b| b.total),
            used_usd: finite(used).max(0.0),
            remaining_usd: budget.as_ref().map(|b| b.remaining),
            models: models_from_rows(activity.unwrap_or(&[]), estimator),
            fetched_at,
        }
    }

    /// Percent of the budget used, `0..=100`; `None` without a budget.
    pub fn used_percent(&self) -> Option<f64> {
        let budget = self.budget_usd.filter(|b| *b > 0.0)?;
        let pct = self.used_usd / budget * 100.0;
        Some(if pct.is_nan() {
            0.0
        } else {
            pct.clamp(0.0, 100.0)
        })
    }

    /// Title of the budget stat card.
    pub const fn budget_stat_title(&self) -> &'static str {
        match self.budget_source {
            Some(BudgetSource::AccountCredits) => "Credits purchased",
            Some(BudgetSource::KeyLimit) => "Key spending limit",
            None => "Credit limit",
        }
    }
}
