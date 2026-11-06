//! Chevron state for daily-budget week browsing.

use serde::{Deserialize, Serialize};

/// How many windows back the daily chart is showing. `0` is the window that
/// contains today; negative values are earlier windows.
///
/// # Examples
///
/// ```
/// use tokenmon_core::usage::WeekOffset;
/// let o = WeekOffset::CURRENT.previous();
/// assert_eq!(o.get(), -1);
/// assert!(o.can_go_next());
/// assert_eq!(o.next(), WeekOffset::CURRENT);
/// assert_eq!(WeekOffset::CURRENT.next(), WeekOffset::CURRENT);
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WeekOffset(i32);

impl WeekOffset {
    /// The window containing today.
    pub const CURRENT: Self = Self(0);

    /// Wraps a raw offset. Positive values are clamped to `0` (the future is not browsable).
    #[must_use]
    pub fn new(raw: i32) -> Self {
        Self(raw.min(0))
    }

    /// The raw offset (`<= 0`).
    #[must_use]
    pub fn get(self) -> i32 {
        self.0
    }

    /// One window further into the past. Unbounded (saturates at `i32::MIN`).
    #[must_use]
    pub fn previous(self) -> Self {
        Self(self.0.saturating_sub(1))
    }

    /// One window toward the present, stopping at the current window.
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1).min(0))
    }

    /// The forward arrow is enabled only while a past window is showing.
    #[must_use]
    pub fn can_go_next(self) -> bool {
        self.0 < 0
    }
}
