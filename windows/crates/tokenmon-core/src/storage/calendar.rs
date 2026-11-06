//! Local-calendar helpers and Apple reference-date conversion.
//!
//! A small private shim over [`Clock`]: day keys (`yyyy-MM-dd`), start of day,
//! calendar-day arithmetic and the Apple-epoch seconds used on disk. All
//! arithmetic is checked, so hostile on-disk values cannot panic.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Timelike, Utc};

use crate::clock::Clock;

/// Seconds between the Unix epoch and the Apple reference date (2001-01-01 UTC).
pub const APPLE_EPOCH_UNIX_SECS: i64 = 978_307_200;

const MIN_YEAR: i32 = 1900;
const MAX_YEAR: i32 = 9000;

/// Converts an instant to Apple reference-date seconds (microsecond precision).
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::storage::to_apple_seconds;
/// let t = Utc.with_ymd_and_hms(2001, 1, 1, 0, 0, 1).unwrap();
/// assert_eq!(to_apple_seconds(t), 1.0);
/// ```
pub fn to_apple_seconds(t: DateTime<Utc>) -> f64 {
    (t.timestamp_micros() - APPLE_EPOCH_UNIX_SECS * 1_000_000) as f64 / 1e6
}

/// Converts Apple reference-date seconds back to an instant.
///
/// Returns `None` for non-finite values or dates outside years 1900..=9000.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::{from_apple_seconds, to_apple_seconds};
/// let t = from_apple_seconds(813_093_922.556).unwrap();
/// assert_eq!(to_apple_seconds(t), 813_093_922.556);
/// assert!(from_apple_seconds(f64::NAN).is_none());
/// assert!(from_apple_seconds(1e300).is_none());
/// ```
pub fn from_apple_seconds(secs: f64) -> Option<DateTime<Utc>> {
    if !secs.is_finite() || secs.abs() > 1.0e11 {
        return None;
    }
    let micros = (secs * 1e6).round() as i64;
    let t =
        DateTime::from_timestamp_micros(micros.checked_add(APPLE_EPOCH_UNIX_SECS * 1_000_000)?)?;
    (MIN_YEAR..=MAX_YEAR).contains(&t.year()).then_some(t)
}

fn local_naive(clock: &dyn Clock, at: DateTime<Utc>) -> Option<NaiveDateTime> {
    let offset = i64::from(clock.local_offset_at(at).local_minus_utc());
    at.naive_utc().checked_add_signed(Duration::seconds(offset))
}

fn utc_from_local(clock: &dyn Clock, local: NaiveDateTime) -> Option<DateTime<Utc>> {
    let offset_at = |guess: NaiveDateTime| {
        let guess_utc = guess.and_utc();
        i64::from(clock.local_offset_at(guess_utc).local_minus_utc())
    };
    let first = local.checked_sub_signed(Duration::seconds(offset_at(local)))?;
    let second = local.checked_sub_signed(Duration::seconds(offset_at(first)))?;
    Some(second.and_utc())
}

fn format_key(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn parse_key(key: &str) -> Option<NaiveDate> {
    let mut parts = key.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

/// The `yyyy-MM-dd` key of the local calendar day containing `at`.
///
/// Falls back to the UTC day for instants whose local time is unrepresentable.
///
/// # Examples
///
/// ```
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_core::{ManualClock};
/// use tokenmon_core::storage::day_key;
/// let t = Utc.with_ymd_and_hms(2026, 9, 15, 3, 0, 0).unwrap();
/// let denver = ManualClock::new(t).with_offset(FixedOffset::west_opt(6 * 3600).unwrap());
/// assert_eq!(day_key(&denver, t), "2026-09-14");
/// ```
pub fn day_key(clock: &dyn Clock, at: DateTime<Utc>) -> String {
    let local = local_naive(clock, at).unwrap_or_else(|| at.naive_utc());
    format_key(local.date())
}

/// The instant of local midnight starting the day named by `key`.
///
/// Derived through noon of that day, like the macOS app, so zones without a
/// 00:00 still resolve. `None` for a malformed key.
///
/// # Examples
///
/// ```
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_core::storage::start_of_day;
/// let clock = ManualClock::from_unix(0).with_offset(FixedOffset::west_opt(6 * 3600).unwrap());
/// let start = start_of_day(&clock, "2026-09-14").unwrap();
/// assert_eq!(start, Utc.with_ymd_and_hms(2026, 9, 14, 6, 0, 0).unwrap());
/// assert!(start_of_day(&clock, "garbage").is_none());
/// ```
pub fn start_of_day(clock: &dyn Clock, key: &str) -> Option<DateTime<Utc>> {
    let date = parse_key(key)?;
    if !(MIN_YEAR..=MAX_YEAR).contains(&date.year()) {
        return None;
    }
    let noon_utc = utc_from_local(clock, date.and_hms_opt(12, 0, 0)?)?;
    let local_noon = local_naive(clock, noon_utc)?;
    utc_from_local(clock, local_noon.date().and_hms_opt(0, 0, 0)?)
}

/// `key` shifted by `days` calendar days. `None` for a malformed key or overflow.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::add_days_to_key;
/// assert_eq!(add_days_to_key("2026-03-01", -1).as_deref(), Some("2026-02-28"));
/// ```
pub fn add_days_to_key(key: &str, days: i64) -> Option<String> {
    let shifted = parse_key(key)?.checked_add_signed(Duration::try_days(days)?)?;
    Some(format_key(shifted))
}

/// `at` shifted by whole local calendar days (DST-safe), keeping the wall-clock time.
pub(crate) fn shift_instant_days(
    clock: &dyn Clock,
    at: DateTime<Utc>,
    days: i64,
) -> Option<DateTime<Utc>> {
    let local = local_naive(clock, at)?;
    let shifted = local.checked_add_signed(Duration::try_days(days)?)?;
    utc_from_local(clock, shifted)
}

/// The local hour (0..=23) of `at`.
pub(crate) fn local_hour(clock: &dyn Clock, at: DateTime<Utc>) -> usize {
    local_naive(clock, at).map_or(0, |l| l.hour() as usize)
}

/// Converts a legacy instant-keyed start-of-day to a day key (spec 1.4.2).
///
/// Picks, among the candidate UTC offsets that would make `instant` a local
/// midnight (`-sod` and `86400 - sod`, limited to -12h..=+14h), the one closest
/// to `preferred_offset` (first wins ties), defaulting to UTC.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::storage::key_for_stored_start_of_day;
/// // Denver midnight (UTC-6) on 2026-09-14 is 06:00 UTC.
/// let t = Utc.with_ymd_and_hms(2026, 9, 14, 6, 0, 0).unwrap();
/// assert_eq!(key_for_stored_start_of_day(t, -6 * 3600), "2026-09-14");
/// ```
pub fn key_for_stored_start_of_day(instant: DateTime<Utc>, preferred_offset: i32) -> String {
    let secs = instant.timestamp_micros().div_euclid(1_000_000);
    let since_midnight = secs.rem_euclid(86_400);
    let offset = [-since_midnight, 86_400 - since_midnight]
        .into_iter()
        .filter(|c| (-43_200..=50_400).contains(c))
        .min_by_key(|c| (c - i64::from(preferred_offset)).abs())
        .unwrap_or(0);
    let shifted = instant
        .naive_utc()
        .checked_add_signed(Duration::seconds(offset))
        .unwrap_or_else(|| instant.naive_utc());
    format_key(shifted.date())
}
