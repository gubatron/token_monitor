//! Exponential backoff for poller retries (spec 01 section 1.2.1).

use std::time::Duration;

/// Exponential backoff for poller retries.
///
/// Starts at zero (no backoff). Each [`record_failure`](Self::record_failure)
/// sets the delay to `initial`, then doubles it up to `maximum`;
/// [`reset`](Self::reset) clears it after a success. `PollingLoop` uses
/// `initial = 30 s`, `maximum = 600 s` (the [`Default`]).
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use tokenmon_core::polling::BackoffTimer;
///
/// let mut b = BackoffTimer::new(Duration::from_secs(15), Duration::from_secs(300));
/// assert_eq!(b.current(), Duration::ZERO);
/// b.record_failure();
/// assert_eq!(b.current(), Duration::from_secs(15));
/// b.record_failure();
/// assert_eq!(b.current(), Duration::from_secs(30));
/// b.reset();
/// assert_eq!(b.current(), Duration::ZERO);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffTimer {
    initial: Duration,
    maximum: Duration,
    current: Duration,
}

impl BackoffTimer {
    /// Creates a timer with no active backoff.
    pub const fn new(initial: Duration, maximum: Duration) -> Self {
        Self {
            initial,
            maximum,
            current: Duration::ZERO,
        }
    }

    /// The current extra wait; zero when the last refresh succeeded.
    pub const fn current(&self) -> Duration {
        self.current
    }

    /// Sets the delay to `initial`, or doubles it, capped at `maximum`.
    pub fn record_failure(&mut self) {
        self.current = if self.current.is_zero() {
            self.initial
        } else {
            self.current.saturating_mul(2).min(self.maximum)
        };
    }

    /// Clears the backoff after a successful refresh.
    pub const fn reset(&mut self) {
        self.current = Duration::ZERO;
    }
}

impl Default for BackoffTimer {
    /// The polling loop's 30 s to 600 s schedule.
    fn default() -> Self {
        Self::new(Duration::from_secs(30), Duration::from_secs(600))
    }
}
