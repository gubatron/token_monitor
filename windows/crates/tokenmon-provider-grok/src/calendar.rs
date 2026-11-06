//! Calendar-day arithmetic over an injected [`Clock`].
//!
//! The daily chart works on local calendar days, DST-safe, like Swift's
//! `Calendar.current`. [`LocalCalendar`] derives the local offset from the
//! clock, so tests pin it with `ManualClock::with_offset`.

use chrono::{DateTime, Days, FixedOffset, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use tokenmon_core::Clock;

/// Local-calendar operations the daily builder needs.
///
/// All methods are total: results that would leave chrono's date range fall
/// back to the input.
pub trait DayCalendar {
    /// Local midnight of the day containing `at`.
    fn start_of_day(&self, at: DateTime<Utc>) -> DateTime<Utc>;
    /// `at` moved by `days` calendar days, keeping the local wall-clock time.
    fn add_days(&self, at: DateTime<Utc>, days: i64) -> DateTime<Utc>;
    /// The local calendar date of `at`.
    fn local_date(&self, at: DateTime<Utc>) -> NaiveDate;
    /// Local noon of the day containing `at`.
    fn noon_of_day(&self, at: DateTime<Utc>) -> DateTime<Utc>;

    /// Whether both instants fall on the same local day.
    fn same_day(&self, a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
        self.local_date(a) == self.local_date(b)
    }

    /// Whole calendar days from `from`'s date to `to`'s date.
    fn days_between(&self, from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
        (self.local_date(to) - self.local_date(from)).num_days()
    }
}

/// [`DayCalendar`] backed by a [`Clock`]'s local offset.
///
/// # Examples
///
/// ```
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_provider_grok::calendar::{DayCalendar, LocalCalendar};
///
/// let clock = ManualClock::from_unix(0).with_offset(FixedOffset::west_opt(8 * 3600).unwrap());
/// let cal = LocalCalendar::new(&clock);
/// let at = Utc.with_ymd_and_hms(2026, 7, 15, 3, 0, 0).unwrap(); // 19:00 on the 14th local
/// assert_eq!(cal.start_of_day(at), Utc.with_ymd_and_hms(2026, 7, 14, 8, 0, 0).unwrap());
/// ```
pub struct LocalCalendar<'a> {
    clock: &'a dyn Clock,
}

impl<'a> LocalCalendar<'a> {
    /// A calendar using `clock`'s local offsets.
    pub fn new(clock: &'a dyn Clock) -> Self {
        Self { clock }
    }

    fn to_local(&self, at: DateTime<Utc>) -> NaiveDateTime {
        at.naive_utc()
            .checked_add_offset(self.clock.local_offset_at(at))
            .unwrap_or_else(|| at.naive_utc())
    }

    /// Converts a local wall-clock time to an instant. The offset is looked
    /// up twice so a time on the far side of a DST change resolves with the
    /// offset in effect there.
    fn to_utc(&self, local: NaiveDateTime, fallback: DateTime<Utc>) -> DateTime<Utc> {
        let resolve = |offset: FixedOffset| {
            local
                .checked_sub_offset(offset)
                .map(|naive| naive.and_utc())
        };
        let first_guess = local.and_utc();
        let Some(first) = resolve(self.clock.local_offset_at(first_guess)) else {
            return fallback;
        };
        resolve(self.clock.local_offset_at(first)).unwrap_or(first)
    }

    fn at_time(&self, at: DateTime<Utc>, hour: u32) -> DateTime<Utc> {
        let date = self.to_local(at).date();
        date.and_hms_opt(hour, 0, 0)
            .map_or(at, |local| self.to_utc(local, at))
    }
}

impl DayCalendar for LocalCalendar<'_> {
    fn start_of_day(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        self.at_time(at, 0)
    }

    fn add_days(&self, at: DateTime<Utc>, days: i64) -> DateTime<Utc> {
        let local = self.to_local(at);
        let moved = if days >= 0 {
            local.checked_add_days(Days::new(days.unsigned_abs()))
        } else {
            local.checked_sub_days(Days::new(days.unsigned_abs()))
        };
        moved.map_or(at, |m| self.to_utc(m, at))
    }

    fn local_date(&self, at: DateTime<Utc>) -> NaiveDate {
        self.to_local(at).date()
    }

    fn noon_of_day(&self, at: DateTime<Utc>) -> DateTime<Utc> {
        self.at_time(at, 12)
    }
}

/// Seconds between two instants as `f64` (saturating on overflow).
pub fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    let delta: TimeDelta = to.signed_duration_since(from);
    delta
        .num_microseconds()
        .map_or_else(|| delta.num_seconds() as f64, |us| us as f64 / 1_000_000.0)
}
