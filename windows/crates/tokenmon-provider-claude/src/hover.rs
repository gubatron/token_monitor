//! The text of the "Models This Month" hover label (Swift
//! `ClaudeModelHoverDetail`): a model's month total and breakdown, and how it
//! compares with last month.
//!
//! Last month is compared only as far as the ledger covers it (see
//! [`ClaudeMonthCoverage`]): a partly recorded month is named by the days it
//! covers (`Oct 9–31`), and a month with no record gives no comparison.

use tokenmon_core::usage::{ChangeUnit, HoverLabelRow, MonthlyUsageChange};
use tokenmon_core::util::Format;

use crate::month::ClaudeMonthCoverage;
use crate::pricing::{cost, display_name};
use crate::summary::{ClaudeCodeUsageSummary, ModelUsageRow};
use crate::tokens::ModelTokens;

/// What the hover label, and the row's accessibility label, say about one of
/// this month's Claude models.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::usage::DayKey;
/// use tokenmon_provider_claude::hover::ClaudeModelHoverDetail;
/// use tokenmon_provider_claude::{ClaudeCodeUsageSummary, ModelTokens};
/// let opus = ModelTokens { output_tokens: 1_000_000, ..Default::default() };
/// let days = BTreeMap::from([(
///     DayKey::from_raw("2026-11-02"),
///     BTreeMap::from([("claude-opus-5-5".to_owned(), opus)]),
/// )]);
/// let now = Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap();
/// let summary = ClaudeCodeUsageSummary::make(&days, now);
/// let detail = ClaudeModelHoverDetail::new(&summary.month_models[0], &summary);
/// assert_eq!(detail.title, "Opus 5.5 · November");
/// assert_eq!(detail.value_text, "≈ $20.00");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeModelHoverDetail {
    /// Model and month, e.g. `Opus 5.5 · November`.
    pub title: String,
    /// This month's tokens.
    pub tokens: i64,
    /// This month's API-rate value (`≈ $1,712.45`), or `No price`.
    pub value_text: String,
    /// Input, output, cache read and cache write tokens this month.
    pub breakdown: Vec<HoverLabelRow>,
    /// Last month's tokens and value, e.g. `October: 1.4M tokens · ≈
    /// $1,500.05`; `None` when there is nothing recorded to show.
    pub previous_line: Option<String>,
    /// The change against last month, e.g. `↑ 14% · +$212.40 vs October`.
    pub change_line: String,
    /// All of the above as one sentence list for a screen reader.
    pub accessibility_label: String,
}

/// A dollar value, with `<$0.01` for one that would otherwise read `$0.00`
/// although something was spent.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::hover::hover_usd;
/// assert_eq!(hover_usd(0.001), "<$0.01");
/// assert_eq!(hover_usd(0.0), "$0.00");
/// assert_eq!(hover_usd(1_712.45), "$1,712.45");
/// ```
#[must_use]
pub fn hover_usd(cost_usd: f64) -> String {
    if cost_usd > 0.0 && cost_usd < 0.005 {
        format!("<{}", Format::usd(0.01))
    } else {
        Format::usd(cost_usd)
    }
}

/// The last-month lines, shown and spoken.
struct Comparison {
    previous_line: Option<String>,
    spoken_previous: Option<String>,
    change_line: String,
    spoken_change: String,
}

impl Comparison {
    fn unchanging(line: String) -> Self {
        Self {
            previous_line: None,
            spoken_previous: None,
            spoken_change: line.clone(),
            change_line: line,
        }
    }

    fn new(model: &ModelUsageRow, summary: &ClaudeCodeUsageSummary) -> Self {
        let previous = &summary.previous_month;
        let period = match summary.previous_month_coverage {
            ClaudeMonthCoverage::Complete => previous.name(),
            ClaudeMonthCoverage::Partial { since } => previous.span(since),
            ClaudeMonthCoverage::None { recorded_since } => {
                let since = recorded_since
                    .map(|d| format!(" (recorded since {})", summary.current_month.short_date(d)))
                    .unwrap_or_default();
                return Self::unchanging(format!("No {} history{since}", previous.name()));
            }
        };
        let Some(previous_tokens) = summary.previous_month_models.get(&model.model_id) else {
            // A model missing from a partly recorded month may have been
            // used on the days before the record starts.
            let complete = summary.previous_month_coverage == ClaudeMonthCoverage::Complete;
            return Self::unchanging(if complete {
                MonthlyUsageChange::New.summary(&period, ChangeUnit::Usd)
            } else {
                format!("No usage recorded {period}")
            });
        };
        Self::against(model, previous_tokens, &period)
    }

    fn against(model: &ModelUsageRow, previous: &ModelTokens, period: &str) -> Self {
        let unit = if model.is_unpriced {
            ChangeUnit::Tokens
        } else {
            ChangeUnit::Usd
        };
        let tokens = format!(
            "{period}: {} tokens",
            Format::tokens(previous.total_tokens())
        );
        let (change, previous_line, spoken_previous) = match unit {
            ChangeUnit::Usd => {
                let previous_cost = cost(previous, &model.model_id).unwrap_or(0.0);
                (
                    MonthlyUsageChange::between(Some(previous_cost), model.cost_usd, unit),
                    format!("{tokens} · ≈ {}", hover_usd(previous_cost)),
                    format!("{tokens}, about {}", hover_usd(previous_cost)),
                )
            }
            ChangeUnit::Tokens => (
                MonthlyUsageChange::between(
                    Some(previous.total_tokens() as f64),
                    model.tokens as f64,
                    unit,
                ),
                tokens.clone(),
                tokens,
            ),
        };
        Self {
            previous_line: Some(previous_line),
            spoken_previous: Some(spoken_previous),
            change_line: change.summary(period, unit),
            spoken_change: change.spoken_summary(period, unit),
        }
    }
}

impl ClaudeModelHoverDetail {
    /// The hover detail of `model`, one of `summary`'s month models.
    #[must_use]
    pub fn new(model: &ModelUsageRow, summary: &ClaudeCodeUsageSummary) -> Self {
        let name = display_name(&model.model_id);
        let month = summary.current_month.name();
        let comparison = Comparison::new(model, summary);
        let t = &model.breakdown;
        let breakdown = HoverLabelRow::breakdown(
            t.input_tokens,
            t.output_tokens,
            t.cache_read_tokens,
            t.cache_write_5m_tokens
                .saturating_add(t.cache_write_1h_tokens),
        );
        let value_text = if model.is_unpriced {
            "No price".to_owned()
        } else {
            format!("≈ {}", hover_usd(model.cost_usd))
        };
        let spoken_value = if model.is_unpriced {
            "no listed price".to_owned()
        } else {
            format!("about {}", hover_usd(model.cost_usd))
        };
        let rows = breakdown
            .iter()
            .map(|r| format!("{} {}", r.label, r.value))
            .collect::<Vec<_>>()
            .join(", ");
        let accessibility_label = [
            Some(format!(
                "{name}, {month}: {} {}, {spoken_value}",
                Format::tokens(model.tokens),
                ClaudeCodeUsageSummary::SOURCE.tokens_unit()
            )),
            Some(rows),
            comparison.spoken_previous,
            Some(comparison.spoken_change),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(". ");
        Self {
            title: format!("{name} · {month}"),
            tokens: model.tokens,
            value_text,
            breakdown,
            previous_line: comparison.previous_line,
            change_line: comparison.change_line,
            accessibility_label,
        }
    }
}
