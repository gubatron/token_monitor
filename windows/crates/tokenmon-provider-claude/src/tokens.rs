//! Token tallies and the calendars that bucket them into days.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A calendar day as `yyyy-MM-dd`.
///
/// A plain alias until the shared `DayKey` from `tokenmon-core` lands.
pub type DayKey = String;

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

/// The calendar used to bucket an instant into a [`DayKey`].
///
/// Identity (not just behavior) matters: the cache keeps one running tally per
/// distinct calendar. `Fixed` exists for tests and constant-offset zones;
/// `Local` follows the machine's current zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayCalendar {
    /// Gregorian calendar in UTC (the ledger's calendar).
    Utc,
    /// The machine's local zone (the daily bars' calendar).
    Local,
    /// A constant offset east of UTC, in seconds.
    Fixed(i32),
}

impl DayCalendar {
    /// The calendar date of `instant` in this calendar.
    pub fn date(self, instant: DateTime<Utc>) -> NaiveDate {
        match self {
            Self::Utc => instant.date_naive(),
            Self::Local => instant.with_timezone(&Local).date_naive(),
            Self::Fixed(secs) => {
                let off = FixedOffset::east_opt(secs)
                    .unwrap_or_else(|| FixedOffset::east_opt(0).expect("zero offset is valid"));
                instant.with_timezone(&off).date_naive()
            }
        }
    }

    /// `yyyy-MM-dd` key of `instant`.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::{TimeZone, Utc};
    /// use tokenmon_provider_claude::DayCalendar;
    /// let t = Utc.with_ymd_and_hms(2026, 10, 10, 20, 0, 0).unwrap();
    /// assert_eq!(DayCalendar::Utc.day_key(t), "2026-10-10");
    /// assert_eq!(DayCalendar::Fixed(9 * 3600).day_key(t), "2026-10-11");
    /// ```
    pub fn day_key(self, instant: DateTime<Utc>) -> DayKey {
        self.date(instant).format("%Y-%m-%d").to_string()
    }
}
