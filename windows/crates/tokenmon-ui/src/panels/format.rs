//! Display formatting shared by every panel view model.
//!
//! These mirror the macOS `Format` helpers (spec 03 section 1.7, spec 02 section 1.7).
//! They live here until `tokenmon-core::util::Format` lands; the panels then
//! switch over and these become thin wrappers or are deleted.

use chrono::{DateTime, FixedOffset, Utc};

/// Clamps a percentage into `0.0..=100.0`; NaN becomes 0.
///
/// ```
/// use tokenmon_ui::panels::format::clamp_percent;
/// assert_eq!(clamp_percent(140.0), 100.0);
/// assert_eq!(clamp_percent(-5.0), 0.0);
/// assert_eq!(clamp_percent(f64::NAN), 0.0);
/// ```
pub fn clamp_percent(p: f64) -> f64 {
    if p.is_nan() { 0.0 } else { p.clamp(0.0, 100.0) }
}

/// `"<Int(round(clamp(p)))>% Used"` (rounding half away from zero).
///
/// ```
/// use tokenmon_ui::panels::format::percent_used;
/// assert_eq!(percent_used(35.5), "36% Used");
/// ```
pub fn percent_used(p: f64) -> String {
    format!("{}% Used", clamp_percent(p).round() as i64)
}

/// `"<Int(round(p))>%"` with a floor of zero (chart labels).
pub fn percent_label(p: f64) -> String {
    format!("{}%", (p.round() as i64).max(0))
}

/// Compact token count: raw below 1000, then `NK`, `N.NM`/`NM`, `N.NB`.
///
/// ```
/// use tokenmon_ui::panels::format::tokens;
/// assert_eq!(tokens(999), "999");
/// assert_eq!(tokens(1_500), "2K");
/// assert_eq!(tokens(999_500), "1.0M");
/// assert_eq!(tokens(12_000_000), "12M");
/// assert_eq!(tokens(2_500_000_000), "2.5B");
/// ```
pub fn tokens(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let k = (n as f64 / 1_000.0).round();
    if k < 1000.0 {
        return format!("{}K", k as u64);
    }
    let m = n as f64 / 1_000_000.0;
    let m1 = (m * 10.0).round() / 10.0;
    if m1 < 10.0 {
        return format!("{m1:.1}M");
    }
    let m0 = m.round();
    if m0 < 1000.0 {
        return format!("{}M", m0 as u64);
    }
    let b = (n as f64 / 1_000_000_000.0 * 10.0).round() / 10.0;
    format!("{b:.1}B")
}

/// en_US currency: `$1,234.56`.
///
/// ```
/// use tokenmon_ui::panels::format::usd;
/// assert_eq!(usd(1234.5), "$1,234.50");
/// assert_eq!(usd(0.0), "$0.00");
/// ```
pub fn usd(v: f64) -> String {
    let v = if v.is_nan() { 0.0 } else { v };
    let cents = (v * 100.0).round() as i64;
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.abs();
    let whole = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}${grouped}.{:02}", cents % 100)
}

/// `"Resets " + EEE dd MMMM h:mma` in the given zone, with lowercase am/pm.
///
/// ```
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_ui::panels::format::reset_caption;
/// let utc = FixedOffset::east_opt(0).unwrap();
/// let t = Utc.with_ymd_and_hms(2026, 7, 16, 20, 25, 0).unwrap();
/// assert_eq!(reset_caption(t, utc), "Resets Thu 16 July 8:25pm");
/// ```
pub fn reset_caption(at: DateTime<Utc>, tz: FixedOffset) -> String {
    format!(
        "Resets {}",
        at.with_timezone(&tz).format("%a %d %B %-I:%M%P")
    )
}

/// Weekly OpenCode caption: `Resets EEE h:mma` (e.g. `Resets Mon 12:00am`).
pub fn reset_caption_short(at: DateTime<Utc>, tz: FixedOffset) -> String {
    format!("Resets {}", at.with_timezone(&tz).format("%a %-I:%M%P"))
}

/// Named relative future time, e.g. `Resets in 2 hours`.
///
/// ```
/// use chrono::{Duration, TimeZone, Utc};
/// use tokenmon_ui::panels::format::reset_caption_relative;
/// let now = Utc.with_ymd_and_hms(2026, 7, 16, 10, 0, 0).unwrap();
/// assert_eq!(reset_caption_relative(now + Duration::minutes(130), now), "Resets in 2 hours");
/// assert_eq!(reset_caption_relative(now + Duration::minutes(1), now), "Resets in 1 minute");
/// ```
pub fn reset_caption_relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (at - now).num_seconds().max(0);
    let (n, unit) = match secs {
        s if s >= 86_400 => (s / 86_400, "day"),
        s if s >= 3_600 => (s / 3_600, "hour"),
        s if s >= 60 => (s / 60, "minute"),
        s => (s, "second"),
    };
    format!("Resets in {n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// Abbreviated age used by the stale-data caption: `5 min`, `2 hr`, `3 days`.
pub fn age_text(since: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - since).num_seconds().max(0);
    match secs {
        s if s >= 86_400 => {
            let d = s / 86_400;
            format!("{d} day{}", if d == 1 { "" } else { "s" })
        }
        s if s >= 3_600 => format!("{} hr", s / 3_600),
        s if s >= 60 => format!("{} min", s / 60),
        s => format!("{s} sec"),
    }
}
