//! Per-day token tallies shown in the daily bars' hover label.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{Calendar, DailyBudgetDay, DayKey};
use crate::util::Format;

/// One model's share of a day.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelTokens {
    /// Display name the usage is listed under.
    pub name: String,
    /// Tokens attributed to the model.
    pub tokens: i64,
}

/// The four token counters of one request (or one model's summed usage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct TokenCounts {
    /// Input tokens.
    pub input: i64,
    /// Output tokens.
    pub output: i64,
    /// Cache-read tokens.
    pub cache_read: i64,
    /// Cache-write tokens.
    pub cache_write: i64,
}

impl TokenCounts {
    /// Sum of the four counters (saturating).
    #[must_use]
    pub fn total(self) -> i64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }
}

/// Tokens used on one day.
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::{DailyTokenUsage, TokenCounts};
/// let mut day = DailyTokenUsage::default();
/// day.add("A", TokenCounts { input: 10, output: 6, ..Default::default() }, Some(1.0));
/// day.add("B", TokenCounts { input: 100, ..Default::default() }, Some(2.5));
/// assert_eq!(day.total_tokens(), 116);
/// assert_eq!(day.cost_usd, Some(3.5));
/// assert_eq!(day.models[0].name, "B");
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DailyTokenUsage {
    /// Input tokens.
    pub input_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Cache-read tokens.
    pub cache_read_tokens: i64,
    /// Cache-write tokens.
    pub cache_write_tokens: i64,
    /// Cost of the priced usage; `None` when none of it has a price.
    pub cost_usd: Option<f64>,
    /// True when some usage has no price, so `cost_usd` leaves it out.
    pub has_unpriced: bool,
    /// Models by tokens, most first, ties by name ascending.
    pub models: Vec<ModelTokens>,
}

impl DailyTokenUsage {
    /// Sum of the four token counters (saturating).
    #[must_use]
    pub fn total_tokens(&self) -> i64 {
        TokenCounts {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
        .total()
    }

    /// Adds one request's (or one model's summed) usage.
    ///
    /// Usage with the same `model` name merges; an empty name counts toward the
    /// totals only. `cost_usd` of `None` marks the day `has_unpriced` when any
    /// tokens were added.
    pub fn add(&mut self, model: &str, counts: TokenCounts, cost_usd: Option<f64>) {
        let tokens = counts.total();
        self.input_tokens = self.input_tokens.saturating_add(counts.input);
        self.output_tokens = self.output_tokens.saturating_add(counts.output);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(counts.cache_read);
        self.cache_write_tokens = self.cache_write_tokens.saturating_add(counts.cache_write);
        if let Some(cost) = cost_usd {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
        } else if tokens > 0 {
            self.has_unpriced = true;
        }
        if tokens <= 0 || model.is_empty() {
            return;
        }
        match self.models.iter_mut().find(|m| m.name == model) {
            Some(existing) => existing.tokens = existing.tokens.saturating_add(tokens),
            None => self.models.push(ModelTokens {
                name: model.to_owned(),
                tokens,
            }),
        }
        self.models
            .sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    }
}

/// `≈ $12.40` for a hover label's value, with a trailing `+` when unpriced
/// usage is left out; `None` when nothing was spent (Swift
/// `DailyBudgetHoverLabel.valueText`).
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::api_value_text;
/// assert_eq!(api_value_text(Some(12.4), false).as_deref(), Some("≈ $12.40"));
/// assert_eq!(api_value_text(Some(12.4), true).as_deref(), Some("≈ $12.40+"));
/// assert_eq!(api_value_text(None, true), None);
/// ```
#[must_use]
pub fn api_value_text(cost_usd: Option<f64>, has_unpriced: bool) -> Option<String> {
    let cost = cost_usd.filter(|c| *c > 0.0)?;
    Some(format!(
        "≈ {}{}",
        Format::usd(cost),
        if has_unpriced { "+" } else { "" }
    ))
}

impl DailyTokenUsage {
    /// The hover label's value text; see [`api_value_text`].
    #[must_use]
    pub fn value_text(&self) -> Option<String> {
        api_value_text(self.cost_usd, self.has_unpriced)
    }
}

/// One label/value row of a hover label's grid.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HoverLabelRow {
    /// Left column, e.g. `Input`.
    pub label: String,
    /// Right column, e.g. `1.2M`.
    pub value: String,
}

impl HoverLabelRow {
    /// A row counting tokens, e.g. `Input` / `1.2M`.
    #[must_use]
    pub fn tokens(label: &str, count: i64) -> Self {
        Self {
            label: label.to_owned(),
            value: Format::tokens(count),
        }
    }

    /// Input, output, cache read and cache write rows, in that order.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::usage::HoverLabelRow;
    /// let rows = HoverLabelRow::breakdown(1_200, 3_400_000, 999, 0);
    /// assert_eq!(rows[0], HoverLabelRow::tokens("Input", 1_200));
    /// assert_eq!(rows[3].value, "0");
    /// ```
    #[must_use]
    pub fn breakdown(input: i64, output: i64, cache_read: i64, cache_write: i64) -> Vec<Self> {
        vec![
            Self::tokens("Input", input),
            Self::tokens("Output", output),
            Self::tokens("Cache read", cache_read),
            Self::tokens("Cache write", cache_write),
        ]
    }
}

/// [`DailyTokenUsage`] per local day, built one request at a time.
#[derive(Debug, Clone)]
pub struct DailyTokenUsageTally<'a> {
    calendar: Calendar<'a>,
    days: BTreeMap<DayKey, DailyTokenUsage>,
}

impl<'a> DailyTokenUsageTally<'a> {
    /// An empty tally bucketing by `calendar`.
    #[must_use]
    pub fn new(calendar: Calendar<'a>) -> Self {
        Self {
            calendar,
            days: BTreeMap::new(),
        }
    }

    /// The calendar used for bucketing.
    #[must_use]
    pub fn calendar(&self) -> &Calendar<'a> {
        &self.calendar
    }

    /// Per-day totals.
    #[must_use]
    pub fn days(&self) -> &BTreeMap<DayKey, DailyTokenUsage> {
        &self.days
    }

    /// Consumes the tally, returning the per-day totals.
    #[must_use]
    pub fn into_days(self) -> BTreeMap<DayKey, DailyTokenUsage> {
        self.days
    }

    /// Adds usage made at `at` to its local day; see [`DailyTokenUsage::add`].
    pub fn add_at(
        &mut self,
        at: DateTime<Utc>,
        model: &str,
        counts: TokenCounts,
        cost_usd: Option<f64>,
    ) {
        let key = DayKey::for_instant(at, &self.calendar);
        self.days
            .entry(key)
            .or_default()
            .add(model, counts, cost_usd);
    }
}

/// The bars with each day's tokens from `tokens_for_day`, which is asked for the
/// [`DayKey`] of each bar's local day; a day it has nothing for keeps its tokens.
#[must_use]
pub fn attaching_tokens(
    days: Vec<DailyBudgetDay>,
    calendar: &Calendar<'_>,
    tokens_for_day: impl Fn(&DayKey) -> Option<DailyTokenUsage>,
) -> Vec<DailyBudgetDay> {
    days.into_iter()
        .map(|day| {
            let key = DayKey::for_instant(day.date, calendar);
            match tokens_for_day(&key) {
                Some(tokens) => DailyBudgetDay {
                    tokens: Some(tokens),
                    ..day
                },
                None => day,
            }
        })
        .collect()
}

/// Like [`attaching_tokens`] with a map; an empty map returns `days` unchanged.
#[must_use]
pub fn attaching_tokens_map<S: std::hash::BuildHasher>(
    days: Vec<DailyBudgetDay>,
    by_day: &HashMap<DayKey, DailyTokenUsage, S>,
    calendar: &Calendar<'_>,
) -> Vec<DailyBudgetDay> {
    if by_day.is_empty() {
        return days;
    }
    attaching_tokens(days, calendar, |k| by_day.get(k).cloned())
}
