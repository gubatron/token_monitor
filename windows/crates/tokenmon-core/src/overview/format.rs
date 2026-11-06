//! Label formatters used by the overview chart.
//!
//! Thin wrappers over the shared [`crate::util::Format`] so there is a single
//! implementation (spec 01 section 1.5).

use crate::util::Format;

/// Compact token count: `999`, `1K`, `1.2M`, `10M`, `2.5B`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::format::format_tokens;
/// assert_eq!(format_tokens(1_500), "2K");
/// assert_eq!(format_tokens(1_250_000), "1.3M");
/// assert_eq!(format_tokens(9_950_000), "10M");
/// assert_eq!(format_tokens(2_500_000_000), "2.5B");
/// ```
#[must_use]
pub fn format_tokens(count: i64) -> String {
    Format::tokens(count)
}

/// Twelve-hour label for the hover tooltip: `12a`, `1p`, `12p`, `11p`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::format::hour_label;
/// assert_eq!(hour_label(0), "12a");
/// assert_eq!(hour_label(12), "12p");
/// assert_eq!(hour_label(15), "3p");
/// ```
#[must_use]
pub fn hour_label(hour: u8) -> String {
    Format::hour_label(i32::from(hour))
}
