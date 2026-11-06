//! `yyyy-MM-dd` day keys independent of time zone.

use std::fmt;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use super::Calendar;

/// A calendar day as a `yyyy-MM-dd` string, independent of time zone.
///
/// Stores persist days in this form so a recorded day keeps meaning the same
/// date after the system time zone changes. Keys sort chronologically as
/// strings. The wrapper does not validate: a malformed key is representable (it
/// may come from disk) and yields `None` from [`DayKey::start_of_day`].
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::usage::{Calendar, DayKey};
///
/// let cal = Calendar::utc();
/// let key = DayKey::for_instant(Utc.with_ymd_and_hms(2026, 9, 15, 3, 0, 0).unwrap(), &cal);
/// assert_eq!(key.as_str(), "2026-09-15");
/// assert_eq!(key.adding_days(-40, &cal).unwrap().as_str(), "2026-08-06");
/// assert!(DayKey::from_raw("garbage").start_of_day(&cal).is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DayKey(String);

impl DayKey {
    /// Wraps a raw stored key without validating it.
    #[must_use]
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Key for a calendar date (year zero-padded to 4 digits, month and day to 2).
    #[must_use]
    pub fn for_date(date: NaiveDate) -> Self {
        Self(format!(
            "{:04}-{:02}-{:02}",
            date.year(),
            date.month(),
            date.day()
        ))
    }

    /// The key for the local calendar day containing `instant`.
    #[must_use]
    pub fn for_instant(instant: DateTime<Utc>, calendar: &Calendar<'_>) -> Self {
        Self::for_date(calendar.date_of(instant))
    }

    /// The date this key names, or `None` when malformed.
    ///
    /// The key is split on `-` and non-integer pieces are dropped; exactly three
    /// integers must remain and form a valid date. (Foundation would roll an
    /// out-of-range month or day over leniently; this rejects it.)
    #[must_use]
    pub fn to_date(&self) -> Option<NaiveDate> {
        let parts: Vec<i32> = self
            .0
            .split('-')
            .filter_map(|p| p.parse::<i32>().ok())
            .collect();
        let [y, m, d] = parts[..] else {
            return None;
        };
        NaiveDate::from_ymd_opt(y, u32::try_from(m).ok()?, u32::try_from(d).ok()?)
    }

    /// Start of the day this key names in `calendar`; `None` for a malformed key.
    #[must_use]
    pub fn start_of_day(&self, calendar: &Calendar<'_>) -> Option<DateTime<Utc>> {
        calendar.start_of_date(self.to_date()?)
    }

    /// The key `days` days after this one (negative for earlier); `None` if malformed or out of range.
    #[must_use]
    pub fn adding_days(&self, days: i64, calendar: &Calendar<'_>) -> Option<DayKey> {
        let date = self
            .to_date()?
            .checked_add_signed(chrono::Duration::try_days(days)?)?;
        // Re-derive through the calendar so the result is what the local day of the shifted start is.
        let start = calendar.start_of_date(date)?;
        Some(Self::for_instant(start, calendar))
    }

    /// The day a stored legacy `startOfDay` instant was recorded for.
    ///
    /// A local midnight in a zone at UTC offset `o` falls `-o` past UTC midnight,
    /// so the instant's UTC time of day pins `o` to one of two values a day apart;
    /// the one in the real range (-12h ..= +14h) is used, and `preferred_offset_secs`
    /// breaks ties (first candidate wins on equal distance).
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::{TimeZone, Utc};
    /// use tokenmon_core::usage::DayKey;
    /// // Tokyo midnight of 2026-07-16 is 2026-07-15T15:00:00Z.
    /// let t = Utc.with_ymd_and_hms(2026, 7, 15, 15, 0, 0).unwrap();
    /// assert_eq!(DayKey::for_stored_start_of_day(t, 9 * 3600).as_str(), "2026-07-16");
    /// ```
    #[must_use]
    pub fn for_stored_start_of_day(instant: DateTime<Utc>, preferred_offset_secs: i64) -> Self {
        let secs = instant.timestamp() + i64::from(instant.timestamp_subsec_nanos() >= 500_000_000);
        let second_of_day = secs.rem_euclid(86_400);
        let best = [-second_of_day, 86_400 - second_of_day]
            .into_iter()
            .filter(|c| (-43_200..=50_400).contains(c))
            .reduce(|best, c| {
                if (c - preferred_offset_secs).abs() < (best - preferred_offset_secs).abs() {
                    c
                } else {
                    best
                }
            })
            .unwrap_or(0);
        let shifted = instant + chrono::Duration::seconds(best);
        Self::for_date(shifted.date_naive())
    }
}

impl fmt::Display for DayKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
