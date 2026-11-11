//! Calendar months of the Claude Code ledger and how much of one the ledger
//! covers (Swift `ClaudeUsageMonth`, `ClaudeMonthCoverage`).

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use tokenmon_core::usage::DayKey;

/// A calendar month of the Claude Code ledger, reckoned in the ledger's
/// calendar (UTC days in the app, so the month turns over at midnight UTC).
///
/// # Examples
///
/// ```
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_provider_claude::month::ClaudeUsageMonth;
/// let utc = FixedOffset::east_opt(0).unwrap();
/// let now = Utc.with_ymd_and_hms(2026, 11, 9, 15, 0, 0).unwrap();
/// let month = ClaudeUsageMonth::containing(now, utc);
/// assert_eq!(month.name(), "November");
/// assert_eq!(month.previous().name(), "October");
/// assert_eq!(month.key_prefix(), "2026-11");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClaudeUsageMonth {
    /// First day of the month in the month's calendar.
    start: NaiveDate,
    /// The calendar's offset, which month and day names are read in.
    offset: FixedOffset,
}

impl ClaudeUsageMonth {
    /// The month containing `instant` in the calendar of `offset`.
    #[must_use]
    pub fn containing(instant: DateTime<Utc>, offset: FixedOffset) -> Self {
        let local = instant.with_timezone(&offset).date_naive();
        Self::of_date(local, offset)
    }

    fn of_date(date: NaiveDate, offset: FixedOffset) -> Self {
        Self {
            start: date.with_day(1).unwrap_or(date),
            offset,
        }
    }

    /// The month before this one.
    #[must_use]
    pub fn previous(self) -> Self {
        let day_before = self.start.pred_opt().unwrap_or(self.start);
        Self::of_date(day_before, self.offset)
    }

    /// First day of the month.
    #[must_use]
    pub fn first_day(self) -> NaiveDate {
        self.start
    }

    /// The `yyyy-MM` prefix every [`DayKey`] of this month starts with.
    #[must_use]
    pub fn key_prefix(self) -> String {
        self.start.format("%Y-%m").to_string()
    }

    /// The [`DayKey`] of the month's first day, as a string.
    #[must_use]
    pub fn first_day_key(self) -> String {
        DayKey::for_date(self.start).as_str().to_owned()
    }

    /// Whether the day `key` names falls in this month.
    #[must_use]
    pub fn contains_day_key(self, key: &DayKey) -> bool {
        key.as_str().starts_with(&self.key_prefix())
    }

    /// The month's English name, e.g. `November`.
    #[must_use]
    pub fn name(self) -> String {
        self.start.format("%B").to_string()
    }

    /// A day of this calendar as a short date, e.g. `Oct 9`.
    #[must_use]
    pub fn short_date(self, date: NaiveDate) -> String {
        date.format("%b %-d").to_string()
    }

    /// The days of this month from `since` to its end, e.g. `Oct 9–31`, or
    /// just `Oct 31` when `since` is the last day.
    #[must_use]
    pub fn span(self, since: NaiveDate) -> String {
        let first = self.short_date(since);
        let last_day = self.days_in_month();
        if since.day() < last_day {
            format!("{first}–{last_day}")
        } else {
            first
        }
    }

    fn days_in_month(self) -> u32 {
        let next = self
            .start
            .checked_add_months(chrono::Months::new(1))
            .unwrap_or(self.start);
        u32::try_from((next - self.start).num_days()).unwrap_or(28)
    }

    /// The calendar offset the month is reckoned in.
    #[must_use]
    pub fn offset(self) -> FixedOffset {
        self.offset
    }
}

/// How much of a month the ledger's saved days cover.
///
/// The ledger only knows the days it has saved. Claude Code deletes old
/// transcripts (after 30 days by default), so usage from before the first scan
/// that is older than that is gone; the first saved day is therefore the
/// earliest day the record can vouch for. A month that starts on or after that
/// day is covered whole; a month the first saved day falls inside is covered
/// only from that day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeMonthCoverage {
    /// The ledger has days from the month's first day or earlier.
    Complete,
    /// The ledger's first saved day, `since`, falls inside the month, so days
    /// before it may be missing.
    Partial {
        /// The first saved day.
        since: NaiveDate,
    },
    /// The ledger has no day in or before the month. `recorded_since` is its
    /// first saved day, `None` when nothing is saved.
    None {
        /// The first saved day, if any.
        recorded_since: Option<NaiveDate>,
    },
}

impl ClaudeMonthCoverage {
    /// The coverage of `month` by a ledger whose earliest saved day is
    /// `earliest` (`None` when the ledger is empty).
    #[must_use]
    pub fn of(month: ClaudeUsageMonth, earliest: Option<&DayKey>) -> Self {
        let Some(earliest) = earliest else {
            return Self::None {
                recorded_since: None,
            };
        };
        // Day keys sort chronologically as strings.
        if earliest.as_str() <= month.first_day_key().as_str() {
            return Self::Complete;
        }
        let since = earliest.to_date();
        match since {
            Some(since) if month.contains_day_key(earliest) => Self::Partial { since },
            _ => Self::None {
                recorded_since: since,
            },
        }
    }
}
