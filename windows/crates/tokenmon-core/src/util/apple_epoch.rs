//! Apple reference-date (2001-01-01T00:00:00Z) conversions.
//!
//! Foundation `Date`s serialize as seconds since this epoch; some persisted
//! stores and test fixtures use that form.

use chrono::{DateTime, Utc};

/// Unix timestamp of the Apple reference date.
pub const APPLE_EPOCH_UNIX_SECS: i64 = 978_307_200;

/// Instant for `secs` seconds since the Apple reference date; `None` for non-finite or out-of-range input.
///
/// # Examples
///
/// ```
/// use tokenmon_core::util::apple_epoch::from_reference_seconds;
/// assert_eq!(from_reference_seconds(0.0).unwrap().timestamp(), 978_307_200);
/// assert!(from_reference_seconds(f64::NAN).is_none());
/// ```
#[must_use]
pub fn from_reference_seconds(secs: f64) -> Option<DateTime<Utc>> {
    if !secs.is_finite() || secs.abs() > 1.0e13 {
        return None;
    }
    let whole = secs.floor();
    let nanos = ((secs - whole) * 1e9).round().min(999_999_999.0) as u32;
    DateTime::from_timestamp(APPLE_EPOCH_UNIX_SECS + whole as i64, nanos)
}

/// Seconds since the Apple reference date, with sub-second precision.
#[must_use]
pub fn to_reference_seconds(at: DateTime<Utc>) -> f64 {
    (at.timestamp() - APPLE_EPOCH_UNIX_SECS) as f64 + f64::from(at.timestamp_subsec_nanos()) / 1e9
}
