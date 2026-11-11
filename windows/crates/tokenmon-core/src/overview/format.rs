//! Label formatters used by the overview chart.
//!
//! Thin wrappers over the shared [`crate::util::Format`] so there is a single
//! implementation (spec 01 section 1.5).

use crate::usage::UsageSource;
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

/// A token count for a tooltip, with where it was counted: `3.4M` for an
/// account's tokens, `3.4M tokens on this PC` for this computer's.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::format::format_tokens_at;
/// use tokenmon_core::usage::UsageSource;
/// assert_eq!(format_tokens_at(3_400_000, UsageSource::Account), "3.4M");
/// assert_eq!(
///     format_tokens_at(3_400_000, UsageSource::ThisComputer),
///     "3.4M tokens on this PC"
/// );
/// ```
#[must_use]
pub fn format_tokens_at(count: i64, source: UsageSource) -> String {
    let count = format_tokens(count);
    match source.location_phrase() {
        Some(phrase) => format!("{count} tokens {phrase}"),
        None => count,
    }
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
