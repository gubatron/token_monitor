//! Month and all-time Claude Code summary (`ClaudeCodeUsageSummary`).

use std::cmp::Ordering;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use tokenmon_core::usage::Calendar;

use crate::month::{ClaudeMonthCoverage, ClaudeUsageMonth};
use crate::pricing::{cost, display_name};
use crate::tokens::{DailyTokens, ModelTokens};

/// Tokens and API-equivalent spend over some period.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UsageTotals {
    /// Total tokens (all five kinds).
    pub tokens: i64,
    /// Summed cost of the priced models.
    pub cost_usd: f64,
    /// Some model with tokens has no price, so `cost_usd` is a lower bound.
    pub has_unpriced: bool,
}

impl UsageTotals {
    /// `$12.50`, with a `+` when some usage is unpriced (`$12.50+`).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_provider_claude::UsageTotals;
    /// let t = UsageTotals { tokens: 0, cost_usd: 12.5, has_unpriced: true };
    /// assert_eq!(t.spend_label(), "$12.50+");
    /// ```
    pub fn spend_label(&self) -> String {
        format!(
            "{}{}",
            format_usd(self.cost_usd),
            if self.has_unpriced { "+" } else { "" }
        )
    }
}

/// en_US currency: `$1,234.56`.
pub fn format_usd(v: f64) -> String {
    let cents = (v.abs() * 100.0).round();
    let cents = if cents.is_finite() { cents as u128 } else { 0 };
    let (whole, frac) = (cents / 100, cents % 100);
    let digits = whole.to_string();
    let grouped = digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(",");
    let sign = if v < 0.0 && cents > 0 { "-" } else { "" };
    format!("{sign}${grouped}.{frac:02}")
}

/// One ranked model row of "Models This Month".
#[derive(Debug, Clone, PartialEq)]
pub struct ModelUsageRow {
    /// Raw model id.
    pub model_id: String,
    /// Total tokens this month.
    pub tokens: i64,
    /// Cost, 0 when unpriced.
    pub cost_usd: f64,
    /// No price known for this model.
    pub is_unpriced: bool,
    /// Share of the month's cost (or tokens when there is no cost), 0-100.
    pub share_percent: f64,
    /// The month's token counts by kind (the hover label's breakdown).
    pub breakdown: ModelTokens,
}

impl ModelUsageRow {
    /// Display name (`Opus 5.5`).
    pub fn display_name(&self) -> String {
        display_name(&self.model_id)
    }
}

/// Claude Code usage this month (UTC) and all time.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCodeUsageSummary {
    /// Current UTC month.
    pub month: UsageTotals,
    /// Everything in the ledger.
    pub all_time: UsageTotals,
    /// Models used this month, ranked by cost then tokens then id.
    pub month_models: Vec<ModelUsageRow>,
    /// The month `month` and `month_models` cover.
    pub current_month: ClaudeUsageMonth,
    /// The calendar month before `current_month`.
    pub previous_month: ClaudeUsageMonth,
    /// How much of `previous_month` the ledger has recorded.
    pub previous_month_coverage: ClaudeMonthCoverage,
    /// Last month's token counts per model id; a model unused last month is
    /// absent.
    pub previous_month_models: BTreeMap<String, ModelTokens>,
}

fn by_model<'a>(
    days: impl Iterator<Item = &'a BTreeMap<String, ModelTokens>>,
) -> BTreeMap<String, ModelTokens> {
    let mut out: BTreeMap<String, ModelTokens> = BTreeMap::new();
    for models in days {
        for (m, t) in models {
            let slot = out.entry(m.clone()).or_default();
            *slot = slot.plus(t);
        }
    }
    out
}

fn totals(models: &BTreeMap<String, ModelTokens>) -> UsageTotals {
    models
        .iter()
        .fold(UsageTotals::default(), |mut acc, (id, t)| {
            acc.tokens = acc.tokens.saturating_add(t.total_tokens());
            match cost(t, id) {
                Some(c) => acc.cost_usd += c,
                None => acc.has_unpriced = true,
            }
            acc
        })
}

fn ranked(models: &BTreeMap<String, ModelTokens>) -> Vec<ModelUsageRow> {
    let mut rows: Vec<ModelUsageRow> = models
        .iter()
        .filter(|(_, t)| t.total_tokens() != 0)
        .map(|(id, t)| {
            let c = cost(t, id);
            ModelUsageRow {
                model_id: id.clone(),
                tokens: t.total_tokens(),
                cost_usd: c.unwrap_or(0.0),
                is_unpriced: c.is_none(),
                share_percent: 0.0,
                breakdown: *t,
            }
        })
        .collect();
    let total_cost: f64 = rows.iter().map(|r| r.cost_usd).sum();
    let total_tokens: i64 = rows.iter().fold(0, |a, r| a.saturating_add(r.tokens));
    for r in &mut rows {
        r.share_percent = if total_cost > 0.0 {
            r.cost_usd / total_cost * 100.0
        } else if total_tokens > 0 {
            r.tokens as f64 / total_tokens as f64 * 100.0
        } else {
            0.0
        };
    }
    rows.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(Ordering::Equal)
            .then(b.tokens.cmp(&a.tokens))
            .then_with(|| a.model_id.cmp(&b.model_id))
    });
    rows
}

impl ClaudeCodeUsageSummary {
    /// Builds the summary from the ledger's UTC `days`. The month turns over
    /// at 00:00 UTC.
    pub fn make(days: &DailyTokens, now: DateTime<Utc>) -> Self {
        Self::make_in(days, now, &Calendar::utc())
    }

    /// Builds the summary for the calendar month containing `now` in
    /// `calendar`. The ledger's calendar decides the month, not the local
    /// zone: with the app's UTC ledger it turns over at midnight UTC.
    pub fn make_in(days: &DailyTokens, now: DateTime<Utc>, calendar: &Calendar<'_>) -> Self {
        let offset = calendar.offset_at(now);
        let current = ClaudeUsageMonth::containing(now, offset);
        let previous = current.previous();
        let in_month = |month: ClaudeUsageMonth| {
            by_model(
                days.iter()
                    .filter(move |(k, _)| month.contains_day_key(k))
                    .map(|(_, v)| v),
            )
        };
        let month_models = in_month(current);
        let all = by_model(days.values());
        let previous_month_models: BTreeMap<String, ModelTokens> = in_month(previous)
            .into_iter()
            .filter(|(_, t)| t.total_tokens() > 0)
            .collect();
        Self {
            month: totals(&month_models),
            all_time: totals(&all),
            month_models: ranked(&month_models),
            current_month: current,
            previous_month: previous,
            previous_month_coverage: ClaudeMonthCoverage::of(previous, days.keys().next()),
            previous_month_models,
        }
    }
}
