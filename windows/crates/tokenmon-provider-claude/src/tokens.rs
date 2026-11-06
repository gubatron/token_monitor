//! Token tallies and the calendars that bucket them into days.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
pub use tokenmon_core::usage::DayKey;

/// Per-day, per-model token counts (`ClaudeDailyTokens`).
pub type DailyTokens = BTreeMap<DayKey, BTreeMap<String, ModelTokens>>;

/// Token counts for one model, summed over some set of responses.
///
/// Cache writes are split by lifetime because they are priced differently.
/// Field order is alphabetical so the derived JSON matches the macOS ledger
/// (sorted keys). Arithmetic saturates: transcripts are untrusted input.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::ModelTokens;
/// let a = ModelTokens { input_tokens: 1, output_tokens: 2, ..Default::default() };
/// assert_eq!(a.total_tokens(), 3);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelTokens {
    /// Prompt tokens served from the prompt cache.
    #[serde(rename = "cacheReadTokens")]
    pub cache_read_tokens: i64,
    /// Prompt tokens written to the 1-hour cache.
    #[serde(rename = "cacheWrite1hTokens")]
    pub cache_write_1h_tokens: i64,
    /// Prompt tokens written to the 5-minute cache.
    #[serde(rename = "cacheWrite5mTokens")]
    pub cache_write_5m_tokens: i64,
    /// Uncached input tokens.
    #[serde(rename = "inputTokens")]
    pub input_tokens: i64,
    /// Output tokens.
    #[serde(rename = "outputTokens")]
    pub output_tokens: i64,
    /// Distinct assistant responses counted.
    #[serde(rename = "responseCount")]
    pub response_count: i64,
}

impl ModelTokens {
    /// Sum of the five token kinds (not the response count).
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_5m_tokens)
            .saturating_add(self.cache_write_1h_tokens)
    }

    /// Fieldwise sum.
    #[must_use]
    pub fn plus(&self, o: &Self) -> Self {
        Self {
            cache_read_tokens: self.cache_read_tokens.saturating_add(o.cache_read_tokens),
            cache_write_1h_tokens: self
                .cache_write_1h_tokens
                .saturating_add(o.cache_write_1h_tokens),
            cache_write_5m_tokens: self
                .cache_write_5m_tokens
                .saturating_add(o.cache_write_5m_tokens),
            input_tokens: self.input_tokens.saturating_add(o.input_tokens),
            output_tokens: self.output_tokens.saturating_add(o.output_tokens),
            response_count: self.response_count.saturating_add(o.response_count),
        }
    }

    /// The larger of each count in `self` and `o`.
    #[must_use]
    pub fn fieldwise_max(&self, o: &Self) -> Self {
        Self {
            cache_read_tokens: self.cache_read_tokens.max(o.cache_read_tokens),
            cache_write_1h_tokens: self.cache_write_1h_tokens.max(o.cache_write_1h_tokens),
            cache_write_5m_tokens: self.cache_write_5m_tokens.max(o.cache_write_5m_tokens),
            input_tokens: self.input_tokens.max(o.input_tokens),
            output_tokens: self.output_tokens.max(o.output_tokens),
            response_count: self.response_count.max(o.response_count),
        }
    }
}
