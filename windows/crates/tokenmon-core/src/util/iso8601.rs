//! Flexible ISO-8601 / RFC 3339 parsing.

use chrono::{DateTime, Utc};

/// Parses an RFC 3339 timestamp with an optional fractional part.
///
/// A time zone (`Z`, `+HH:MM` or `+HHMM`) is **required**. Fractional precision
/// is kept (to nanoseconds). Date-only strings, strings without a zone and
/// garbage give `None`. Never panics.
///
/// # Examples
///
/// ```
/// use tokenmon_core::util::parse_flexible;
/// assert_eq!(parse_flexible("2026-07-16T20:25:00Z").unwrap().timestamp(), 1_784_233_500);
/// assert_eq!(parse_flexible("2026-07-16T20:25:00+0530").unwrap().timestamp(), 1_784_213_700);
/// assert!(parse_flexible("2026-07-16").is_none());
/// assert!(parse_flexible("2026-07-16T20:25:00").is_none());
/// ```
#[must_use]
pub fn parse_flexible(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}
