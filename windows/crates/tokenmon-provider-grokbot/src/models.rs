//! Grokbot snapshot types (spec 02 section 2.4).

use chrono::{DateTime, Utc};
use tokenmon_core::usage::Percent;
use tokenmon_core::usage::{Calendar, DailyBudget, UsagePool};

/// Fallback period length, in days, when the provider's span is not weekly.
pub const DEFAULT_PERIOD_DAYS: i64 = 7;

/// Who pays for the Grokbot allowance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrokbotEntitlement {
    /// Included in the user's Cursor plan.
    Cursor,
    /// Funded by a SuperGrok plan; carries the plan label (may be empty).
    SuperGrok(String),
}

impl GrokbotEntitlement {
    /// `captionText`: `via Cursor`, `via <label>`, or `via SuperGrok`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_provider_grokbot::GrokbotEntitlement as E;
    /// assert_eq!(E::Cursor.caption_text(), "via Cursor");
    /// assert_eq!(E::SuperGrok("SuperGrok Heavy".into()).caption_text(), "via SuperGrok Heavy");
    /// assert_eq!(E::SuperGrok(String::new()).caption_text(), "via SuperGrok");
    /// ```
    pub fn caption_text(&self) -> String {
        match self {
            Self::Cursor => "via Cursor".to_owned(),
            Self::SuperGrok(label) if label.is_empty() => "via SuperGrok".to_owned(),
            Self::SuperGrok(label) => format!("via {label}"),
        }
    }
}

/// One successful Grokbot refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokbotSnapshot {
    /// When the data was fetched.
    pub fetched_at: DateTime<Utc>,
    /// Used percent, clamped to 0..=100.
    pub used_percent: f64,
    /// Start of the current allowance period, if reported.
    pub period_start: Option<DateTime<Utc>>,
    /// Reset instant, if reported. Never substituted when missing.
    pub resets_at: Option<DateTime<Utc>>,
    /// Who funds the allowance.
    pub entitlement: GrokbotEntitlement,
    /// Cursor account email.
    pub account_email: Option<String>,
    /// False when the plan has no included allowance (on-demand billing).
    pub has_included_allowance: bool,
}

impl GrokbotSnapshot {
    /// `clamp(100 - used)`.
    pub fn remaining_percent(&self) -> f64 {
        Percent::clamp(100.0 - self.used_percent)
    }

    /// The raw span between period start and reset in local calendar days;
    /// `None` when either end is missing or the reset is not after the start.
    pub fn reported_span_days(&self, cal: &Calendar<'_>) -> Option<i64> {
        match (self.period_start, self.resets_at) {
            (Some(start), Some(end)) if end > start => {
                Some(DailyBudget::days_in_billing_cycle(start, end, cal))
            }
            _ => None,
        }
    }

    /// The reported span when it is 6 to 8 days, otherwise 7. This covers a
    /// missing end, a same-day collapse, a five-day span and a fortnight.
    pub fn days_in_period(&self, cal: &Calendar<'_>) -> i64 {
        self.reported_span_days(cal)
            .filter(|d| (6..=8).contains(d))
            .unwrap_or(DEFAULT_PERIOD_DAYS)
    }

    /// Pool cadence from the raw span: 25+ days monthly, up to 10 weekly,
    /// otherwise mixed. No span counts as 7 (weekly).
    pub fn usage_pool(&self, cal: &Calendar<'_>) -> UsagePool {
        let days = self.reported_span_days(cal).unwrap_or(DEFAULT_PERIOD_DAYS);
        if days >= 25 {
            UsagePool::Monthly
        } else if days <= 10 {
            UsagePool::Weekly
        } else {
            UsagePool::Mixed
        }
    }
}
