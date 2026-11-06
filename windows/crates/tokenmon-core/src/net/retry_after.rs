//! `Retry-After` parsing (spec 01 section 1.1.2).

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, RETRY_AFTER};

/// Upper bound for any `Retry-After` hint, in seconds.
pub const MAX_RETRY_AFTER_SECS: f64 = 3600.0;

/// Parses a `Retry-After` header value into seconds in `0..=3600`.
///
/// 1. Surrounding whitespace is trimmed; empty gives `None`.
/// 2. A value that parses as a float (including `inf`, `infinity`, exponents and
///    a leading sign, like Swift's `Double(String)`) is clamped to the range;
///    NaN gives `None`.
/// 3. Otherwise an HTTP date (`Wed, 21 Oct 2015 07:28:00 GMT`) gives
///    `date - now` clamped to the range; anything else gives `None`.
///
/// Total over arbitrary input: never panics, never returns NaN or a value
/// outside the range.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::net::parse_retry_after;
///
/// let now = Utc.with_ymd_and_hms(2015, 10, 21, 7, 27, 0).unwrap();
/// assert_eq!(parse_retry_after("120", now), Some(120.0));
/// assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now), Some(60.0));
/// assert_eq!(parse_retry_after("-5", now), Some(0.0));
/// assert_eq!(parse_retry_after("inf", now), Some(3600.0));
/// assert_eq!(parse_retry_after("nan", now), None);
/// ```
pub fn parse_retry_after(value: &str, now: DateTime<Utc>) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(secs) = trimmed.parse::<f64>() {
        return (!secs.is_nan()).then(|| secs.clamp(0.0, MAX_RETRY_AFTER_SECS));
    }
    let date = DateTime::parse_from_rfc2822(trimmed)
        .ok()?
        .with_timezone(&Utc);
    let secs = date.signed_duration_since(now).num_milliseconds() as f64 / 1000.0;
    Some(secs.clamp(0.0, MAX_RETRY_AFTER_SECS))
}

/// Reads and parses the `Retry-After` header of a response.
///
/// A missing or non-text header gives `None`.
pub fn retry_after_from_headers(headers: &HeaderMap, now: DateTime<Utc>) -> Option<f64> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?;
    parse_retry_after(raw, now)
}
