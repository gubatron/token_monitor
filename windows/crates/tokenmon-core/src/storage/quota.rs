//! `QuotaWindow` and `QuotaWindowTransition` (spec 1.3.5).

use chrono::{DateTime, Duration, Utc};

/// The default quota period used when a provider does not say otherwise.
pub const DEFAULT_PERIOD_DAYS: i64 = 7;

/// Allowed clock skew when deciding whether the old window is still running.
const SKEW_SECS: i64 = 300;
/// How far a boundary must move to count as "moved".
const MOVED_THRESHOLD_SECS: i64 = 3600;
/// Minimum usage drop (points) that makes a moved window an early reset.
const EARLY_RESET_DROP: f64 = super::delta::RESET_DROP_FLOOR;

/// The bounds of a quota window as the provider reports them.
///
/// Grokbot and Cursor send both; Claude sends only `resets_at`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::QuotaWindow;
/// assert_eq!(QuotaWindow::default().start, None);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QuotaWindow {
    /// When the window began, if reported.
    pub start: Option<DateTime<Utc>>,
    /// When the window resets, if reported.
    pub resets_at: Option<DateTime<Utc>>,
}

/// What happened between two observations of a quota window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuotaWindowTransition {
    /// Same window (or not enough information to say otherwise).
    None,
    /// The old window ended naturally and the next one started.
    Rollover,
    /// The provider reset the window before its scheduled end.
    EarlyReset,
}

impl QuotaWindowTransition {
    /// Whether `next_reset` is a full-period advance over `prev_reset`:
    /// later, by at least half of `period_days`.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::{Duration, TimeZone, Utc};
    /// use tokenmon_core::storage::QuotaWindowTransition as T;
    /// let a = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    /// assert!(T::is_rollover(Some(a), Some(a + Duration::days(7)), 7));
    /// assert!(!T::is_rollover(Some(a), Some(a + Duration::days(1)), 7));
    /// ```
    pub fn is_rollover(
        prev_reset: Option<DateTime<Utc>>,
        next_reset: Option<DateTime<Utc>>,
        period_days: i64,
    ) -> bool {
        let (Some(prev), Some(next)) = (prev_reset, next_reset) else {
            return false;
        };
        let half_period_ms = period_days.clamp(1, 36_500) * 86_400 * 500;
        next > prev && (next - prev).num_milliseconds() >= half_period_ms
    }

    /// Classifies a move from `previous` to `next`.
    ///
    /// Needs `resets_at` on both sides. If the old window had already ended
    /// (its reset is not after `now` plus a 5 minute skew) it is a rollover when
    /// [`Self::is_rollover`] holds. Otherwise it is an early reset only when
    /// usage dropped by at least 5 points and the start moved forward, or the
    /// reset instant moved, by at least an hour.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::{Duration, TimeZone, Utc};
    /// use tokenmon_core::storage::{QuotaWindow, QuotaWindowTransition as T};
    /// let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    /// let old = QuotaWindow { start: None, resets_at: Some(now + Duration::days(3)) };
    /// let new = QuotaWindow { start: None, resets_at: Some(now + Duration::days(7)) };
    /// assert_eq!(T::classify(old, Some(60.0), new, 2.0, 7, now), T::EarlyReset);
    /// assert_eq!(T::classify(old, Some(60.0), new, 58.0, 7, now), T::None);
    /// ```
    pub fn classify(
        previous: QuotaWindow,
        previous_used_percent: Option<f64>,
        next: QuotaWindow,
        used_percent: f64,
        period_days: i64,
        now: DateTime<Utc>,
    ) -> Self {
        let (Some(prev_reset), Some(next_reset)) = (previous.resets_at, next.resets_at) else {
            return Self::None;
        };
        let old_still_running = prev_reset > now + Duration::seconds(SKEW_SECS);
        if !old_still_running {
            return if Self::is_rollover(Some(prev_reset), Some(next_reset), period_days) {
                Self::Rollover
            } else {
                Self::None
            };
        }
        let Some(prev_used) = previous_used_percent else {
            return Self::None;
        };
        if prev_used - used_percent < EARLY_RESET_DROP {
            return Self::None;
        }
        let threshold = Duration::seconds(MOVED_THRESHOLD_SECS);
        let start_moved = match (previous.start, next.start) {
            (Some(p), Some(n)) => n - p >= threshold,
            _ => false,
        };
        let reset_moved = (next_reset - prev_reset).abs() >= threshold;
        if start_moved || reset_moved {
            Self::EarlyReset
        } else {
            Self::None
        }
    }
}
