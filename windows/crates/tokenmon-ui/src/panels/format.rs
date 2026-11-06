//! Display formatting shared by every panel view model.
//!
//! Token, USD and reset-date formatting delegate to
//! [`tokenmon_core::util::Format`]; what remains here are the panel-only
//! helpers (percent labels, relative and age wording) core does not provide.

use chrono::{DateTime, FixedOffset, Utc};
use tokenmon_core::util::Format;

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
    Format::tokens(i64::try_from(n).unwrap_or(i64::MAX))
}

/// en_US currency: `$1,234.56`.
///
/// ```
/// use tokenmon_ui::panels::format::usd;
/// assert_eq!(usd(1234.5), "$1,234.50");
/// assert_eq!(usd(0.0), "$0.00");
/// ```
pub fn usd(v: f64) -> String {
    Format::usd(v)
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
    Format::reset_caption(at, Format::DEFAULT_RESET_PATTERN, tz)
}

/// Weekly OpenCode caption: `Resets EEE h:mma` (e.g. `Resets Mon 12:00am`).
pub fn reset_caption_short(at: DateTime<Utc>, tz: FixedOffset) -> String {
    Format::reset_caption(at, "EEE h:mma", tz)
}

/// Relative reset wording matching SwiftUI's
/// `date.formatted(.relative(presentation: .named))`, prefixed with `Resets `.
///
/// The value is rounded half-up to the nearest whole unit and the unit is the
/// largest whose rounded value still fits: seconds below a minute, minutes
/// below an hour, hours below a day, then days (`tomorrow` / `yesterday` for
/// one). Zero gives `now`; past dates read `N units ago`.
///
/// ```
/// use chrono::{Duration, TimeZone, Utc};
/// use tokenmon_ui::panels::format::reset_caption_relative;
/// let now = Utc.with_ymd_and_hms(2026, 7, 16, 10, 0, 0).unwrap();
/// assert_eq!(reset_caption_relative(now + Duration::minutes(130), now), "Resets in 2 hours");
/// assert_eq!(reset_caption_relative(now + Duration::seconds(3599), now), "Resets in 1 hour");
/// assert_eq!(reset_caption_relative(now, now), "Resets now");
/// ```
pub fn reset_caption_relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = {
        let ms = (at - now).num_milliseconds();
        (ms + ms.signum() * 500) / 1_000
    };
    let abs = secs.abs();
    let round_div = |unit: i64| (abs * 2 + unit) / (2 * unit);
    let (n, unit) = if abs < 60 {
        (abs, "second")
    } else if round_div(60) < 60 {
        (round_div(60), "minute")
    } else if round_div(3_600) < 24 {
        (round_div(3_600), "hour")
    } else {
        (round_div(86_400), "day")
    };
    let phrase = match (secs, n, unit) {
        (0, _, _) => "now".to_owned(),
        (s, 1, "day") if s > 0 => "tomorrow".to_owned(),
        (_, 1, "day") => "yesterday".to_owned(),
        (s, n, unit) => {
            let plural = if n == 1 { "" } else { "s" };
            if s > 0 {
                format!("in {n} {unit}{plural}")
            } else {
                format!("{n} {unit}{plural} ago")
            }
        }
    };
    format!("Resets {phrase}")
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
