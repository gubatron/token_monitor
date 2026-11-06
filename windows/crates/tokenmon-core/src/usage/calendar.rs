//! Injected local calendar: day boundaries without the host time zone.

use std::fmt;

use chrono::{
    DateTime, Datelike, Days, FixedOffset, Months, NaiveDate, NaiveDateTime, NaiveTime, Utc,
};

use crate::Clock;

/// A Gregorian calendar in a specific local time zone, supplied by the caller.
///
/// Either a fixed UTC offset ([`Calendar::fixed`], [`Calendar::utc`]) or a
/// [`Clock`] whose `local_offset_at` may vary with DST ([`Calendar::local`]).
/// Instants are `DateTime<Utc>`; "day" always means the local calendar day.
///
/// # Examples
///
/// ```
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_core::usage::Calendar;
///
/// let denver = Calendar::fixed(FixedOffset::west_opt(6 * 3600).unwrap());
/// let t = Utc.with_ymd_and_hms(2026, 9, 15, 5, 0, 0).unwrap(); // 23:00 on the 14th in Denver
/// assert_eq!(denver.date_of(t).to_string(), "2026-09-14");
/// assert_eq!(denver.start_of_day(t), Utc.with_ymd_and_hms(2026, 9, 14, 6, 0, 0).unwrap());
/// ```
#[derive(Clone, Copy)]
pub enum Calendar<'a> {
    /// A constant offset (no DST).
    Fixed(FixedOffset),
    /// Offsets come from an injected clock, so DST is honoured.
    Clock(&'a dyn Clock),
}

impl fmt::Debug for Calendar<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixed(o) => write!(f, "Calendar::Fixed({o})"),
            Self::Clock(_) => f.write_str("Calendar::Clock(..)"),
        }
    }
}

impl<'a> Calendar<'a> {
    /// UTC calendar.
    #[must_use]
    pub fn utc() -> Calendar<'static> {
        Calendar::Fixed(FixedOffset::east_opt(0).expect("zero offset is valid"))
    }

    /// Calendar with a constant offset.
    #[must_use]
    pub fn fixed(offset: FixedOffset) -> Calendar<'static> {
        Calendar::Fixed(offset)
    }

    /// Calendar following `clock.local_offset_at`.
    #[must_use]
    pub fn local(clock: &'a dyn Clock) -> Self {
        Self::Clock(clock)
    }

    /// The UTC offset in effect at `instant`.
    #[must_use]
    pub fn offset_at(&self, instant: DateTime<Utc>) -> FixedOffset {
        match self {
            Self::Fixed(o) => *o,
            Self::Clock(c) => c.local_offset_at(instant),
        }
    }

    /// Local wall-clock time at `instant`.
    #[must_use]
    pub fn local_naive(&self, instant: DateTime<Utc>) -> NaiveDateTime {
        instant
            .naive_utc()
            .checked_add_offset(self.offset_at(instant))
            .unwrap_or_else(|| instant.naive_utc())
    }

    /// Local calendar date of `instant`.
    #[must_use]
    pub fn date_of(&self, instant: DateTime<Utc>) -> NaiveDate {
        self.local_naive(instant).date()
    }

    /// Converts a local wall-clock time to an instant (nearest valid mapping across DST gaps).
    #[must_use]
    pub fn resolve(&self, local: NaiveDateTime) -> Option<DateTime<Utc>> {
        let first = self.offset_at(local.and_utc());
        let guess = local.checked_sub_offset(first)?.and_utc();
        let second = self.offset_at(guess);
        Some(local.checked_sub_offset(second)?.and_utc())
    }

    /// Instant of local midnight on `date`.
    #[must_use]
    pub fn start_of_date(&self, date: NaiveDate) -> Option<DateTime<Utc>> {
        self.resolve(date.and_time(NaiveTime::MIN))
    }

    /// Instant of the start of the local day containing `instant`.
    #[must_use]
    pub fn start_of_day(&self, instant: DateTime<Utc>) -> DateTime<Utc> {
        self.start_of_date(self.date_of(instant)).unwrap_or(instant)
    }

    /// `instant` shifted by `days` calendar days, keeping the local wall-clock time.
    #[must_use]
    pub fn add_days(&self, instant: DateTime<Utc>, days: i64) -> Option<DateTime<Utc>> {
        let local = self.local_naive(instant);
        let magnitude = Days::new(days.unsigned_abs());
        let shifted = if days >= 0 {
            local.checked_add_days(magnitude)?
        } else {
            local.checked_sub_days(magnitude)?
        };
        self.resolve(shifted)
    }

    /// `instant` shifted by `months` calendar months, clamping the day to the target month's length.
    #[must_use]
    pub fn add_months(&self, instant: DateTime<Utc>, months: i32) -> Option<DateTime<Utc>> {
        let local = self.local_naive(instant);
        let magnitude = Months::new(months.unsigned_abs());
        let shifted = if months >= 0 {
            local.checked_add_months(magnitude)?
        } else {
            local.checked_sub_months(magnitude)?
        };
        self.resolve(shifted)
    }

    /// Whole local calendar days from the day of `from` to the day of `to`.
    #[must_use]
    pub fn day_diff(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
        (self.date_of(to) - self.date_of(from)).num_days()
    }

    /// Weekday of `instant`'s local date, `1 = Sunday … 7 = Saturday` (Foundation convention).
    #[must_use]
    pub fn weekday_sunday_first(&self, instant: DateTime<Utc>) -> u32 {
        self.date_of(instant).weekday().num_days_from_sunday() + 1
    }
}
