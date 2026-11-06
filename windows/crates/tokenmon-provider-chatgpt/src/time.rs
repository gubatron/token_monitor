//! Total conversions between epoch seconds (`f64`) and [`DateTime<Utc>`].

use chrono::{DateTime, Utc};

/// Seconds since the epoch as `f64` (sub-second precision kept).
pub fn epoch_secs(at: DateTime<Utc>) -> f64 {
    at.timestamp() as f64 + f64::from(at.timestamp_subsec_nanos()) / 1e9
}

/// `Date(timeIntervalSince1970:)`: `None` for NaN, infinities and instants
/// chrono cannot represent (Swift would carry them; no UI can show them).
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::time::from_epoch_secs;
/// assert_eq!(from_epoch_secs(1_787_300_000.0).unwrap().timestamp(), 1_787_300_000);
/// assert!(from_epoch_secs(f64::NAN).is_none());
/// assert!(from_epoch_secs(1e300).is_none());
/// ```
pub fn from_epoch_secs(secs: f64) -> Option<DateTime<Utc>> {
    if !secs.is_finite() || secs.abs() > 8.0e12 {
        return None;
    }
    let whole = secs.floor();
    let nanos = ((secs - whole) * 1e9).clamp(0.0, 999_999_999.0) as u32;
    DateTime::from_timestamp(whole as i64, nanos)
}
