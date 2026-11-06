//! The 30-day hourly history (spec 01 section 4.3).
//!
//! The store is pure: [`OverviewHistory::decode`] and [`OverviewHistory::encode`]
//! convert to and from the file contents, and the caller (the app's storage
//! layer) does the I/O. [`OverviewHistory::load`] and [`OverviewHistory::save`]
//! are thin, atomic file helpers.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};

use super::series::OverviewDaySeries;

/// Days kept, counting today.
pub const RETENTION_DAYS: u64 = 30;

/// File name of the history file.
pub const FILE_NAME: &str = "activity_overview_hourly_history.dat";

/// Storage key of the history payload.
pub const STORAGE_KEY: &str = "overview_hourly_history";

/// Largest file [`OverviewHistory::decode`] will parse. Real files are a few
/// tens of KiB; anything bigger is treated as corrupt.
pub const MAX_DECODE_BYTES: usize = 4 * 1024 * 1024;

/// On-disk shape. Fields are declared in key order so output stays sorted.
#[derive(Serialize, Deserialize)]
struct Wire {
    days: BTreeMap<String, OverviewDaySeries>,
    #[serde(default)]
    version: u32,
}

const WIRE_VERSION: u32 = 1;

fn day_key(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

/// Parses a canonical `yyyy-MM-dd` key; non-canonical spellings are rejected so
/// a key round-trips exactly.
fn parse_key(key: &str) -> Option<NaiveDate> {
    let day = NaiveDate::parse_from_str(key, "%Y-%m-%d").ok()?;
    (day_key(day) == key).then_some(day)
}

/// Per-day hourly history, merged by maximum and capped at [`RETENTION_DAYS`].
///
/// # Examples
///
/// ```
/// use chrono::NaiveDate;
/// use tokenmon_core::overview::{OverviewDaySeries, OverviewHistory, OverviewSeries::Grok};
///
/// let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
/// let mut h = OverviewHistory::default();
/// // An empty day is never created.
/// assert!(!h.record(&OverviewDaySeries::default(), day));
/// let s = OverviewDaySeries::default().with_weights(Grok, vec![1.0]);
/// assert!(h.record(&s, day));
/// let reloaded = OverviewHistory::decode(&h.encode());
/// assert_eq!(reloaded, h);
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverviewHistory {
    days: BTreeMap<NaiveDate, OverviewDaySeries>,
}

impl OverviewHistory {
    /// Decodes file contents. Any failure (bad JSON, oversized input, unknown
    /// series) yields an empty history; it never errors. Keys that are not
    /// canonical `yyyy-MM-dd` are dropped, arrays are truncated to 24 hours and
    /// bad numbers are zeroed.
    pub fn decode(text: &str) -> Self {
        if text.len() > MAX_DECODE_BYTES {
            return Self::default();
        }
        let Ok(wire) = serde_json::from_str::<Wire>(text) else {
            return Self::default();
        };
        let days = wire
            .days
            .into_iter()
            .filter_map(|(k, v)| Some((parse_key(&k)?, v.normalized())))
            .collect();
        Self { days }
    }

    /// Encodes with sorted keys and `"version":1`.
    pub fn encode(&self) -> String {
        let wire = Wire {
            days: self
                .days
                .iter()
                .map(|(d, s)| (day_key(*d), s.clone()))
                .collect(),
            version: WIRE_VERSION,
        };
        serde_json::to_string(&wire).expect("history is always serializable")
    }

    /// Merges `series` into `date` and trims days past the retention window.
    /// Returns `true` when anything changed (the caller then saves).
    pub fn record(&mut self, series: &OverviewDaySeries, date: NaiveDate) -> bool {
        let mut changed = false;
        let existing = self.days.get(&date);
        if series.has_activity() || existing.is_some() {
            let merged = existing.cloned().unwrap_or_default().merging(series);
            if existing != Some(&merged) {
                self.days.insert(date, merged);
                changed = true;
            }
        }
        self.trim(date) | changed
    }

    fn trim(&mut self, today: NaiveDate) -> bool {
        let Some(oldest) = today.checked_sub_days(Days::new(RETENTION_DAYS - 1)) else {
            return false;
        };
        let before = self.days.len();
        self.days = self.days.split_off(&oldest);
        self.days.len() != before
    }

    /// The saved series for `date`.
    pub fn series(&self, date: NaiveDate) -> Option<&OverviewDaySeries> {
        self.days.get(&date)
    }

    /// The earliest saved day, if any.
    pub fn earliest_day(&self) -> Option<NaiveDate> {
        self.days.keys().next().copied()
    }

    /// Number of saved days.
    pub fn len(&self) -> usize {
        self.days.len()
    }

    /// `true` when no day is saved.
    pub fn is_empty(&self) -> bool {
        self.days.is_empty()
    }

    /// Reads and decodes `path`; a missing or unreadable file gives an empty
    /// history.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map_or_else(|_| Self::default(), |t| Self::decode(&t))
    }

    /// Writes the history atomically (temp file in the same folder, then rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.encode())?;
        std::fs::rename(&tmp, path)
    }
}
