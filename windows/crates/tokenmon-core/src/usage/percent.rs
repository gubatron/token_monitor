//! Percentage clamping and the shared noise thresholds.

/// Namespace for percentage constants and clamping used by renderers and model math.
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::Percent;
/// assert_eq!(Percent::clamp(120.0), 100.0);
/// assert_eq!(Percent::clamp(-3.0), 0.0);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Percent;

impl Percent {
    /// Growth smaller than this is rounding noise and is not recorded.
    pub const NOISE_FLOOR: f64 = 0.05;

    /// A drop at least this large is a quota-window reset; smaller drops are noise.
    pub const RESET_DROP_FLOOR: f64 = 5.0;

    /// Clamps `value` into `0.0..=100.0`. NaN clamps to `0.0`.
    #[must_use]
    pub fn clamp(value: f64) -> f64 {
        if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 100.0)
        }
    }
}
