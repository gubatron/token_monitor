//! The percentage-point delta rule shared by the hourly and daily stores.

/// Deltas below this many percentage points are noise and ignored.
pub const NOISE_FLOOR: f64 = 0.05;
/// A downward move of at least this many points is a window reset.
pub const RESET_DROP_FLOOR: f64 = 5.0;

/// How many percentage points to credit when usage moves `previous` -> `used`.
///
/// * growth: the difference;
/// * a drop of at least [`RESET_DROP_FLOOR`] (window reset): `used`, the
///   post-reset value;
/// * a smaller drop: `0` (noise).
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::credited_delta;
/// assert_eq!(credited_delta(25.0, 10.0), 15.0);
/// assert_eq!(credited_delta(25.0, 60.0), 25.0);
/// assert_eq!(credited_delta(49.9, 50.0), 0.0);
/// ```
pub fn credited_delta(used: f64, previous: f64) -> f64 {
    if used >= previous {
        used - previous
    } else if previous - used >= RESET_DROP_FLOOR {
        used
    } else {
        0.0
    }
}
