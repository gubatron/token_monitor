//! Label formatters used by the overview chart.
//!
//! Spec 01 section 1.5 (`Format.tokens`, `Format.hourLabel`). They live here
//! until the shared utilities module lands; swap the bodies for re-exports then.

/// Rounds half away from zero to `dec` decimals.
fn round_to(x: f64, dec: i32) -> f64 {
    let p = 10f64.powi(dec);
    (x * p).round() / p
}

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
pub fn format_tokens(count: i64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    let v = count as f64;
    let k = (v / 1000.0).round();
    if k < 1000.0 {
        return format!("{k:.0}K");
    }
    let m = v / 1e6;
    let dec = usize::from(round_to(m, 1) < 10.0);
    let shown = round_to(m, i32::from(dec == 1));
    if shown < 1000.0 {
        return format!("{shown:.dec$}M");
    }
    format!("{:.1}B", round_to(v / 1e9, 1))
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
pub fn hour_label(hour: u8) -> String {
    match hour {
        0 => "12a".to_owned(),
        12 => "12p".to_owned(),
        1..=11 => format!("{hour}a"),
        _ => format!("{}p", hour % 12),
    }
}
