//! Injectable time source.
//!
//! Every piece of logic that depends on "now" or on the local time zone takes a
//! [`Clock`] so tests are deterministic and never sleep.

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local, TimeZone, Utc};

/// Source of the current instant and of the user's local UTC offset.
///
/// Day bucketing (`DayKey`), budgets and hourly history all depend on the
/// *local* calendar, so the offset is part of the contract.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::{Clock, ManualClock};
///
/// let clock = ManualClock::new(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
/// assert_eq!(clock.now().to_rfc3339(), "2026-01-01T00:00:00+00:00");
/// ```
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> DateTime<Utc>;

    /// The local UTC offset in effect at `at` (DST aware for the real clock).
    fn local_offset_at(&self, at: DateTime<Utc>) -> FixedOffset;

    /// The current instant expressed in local time.
    fn now_local(&self) -> DateTime<FixedOffset> {
        let now = self.now();
        now.with_timezone(&self.local_offset_at(now))
    }
}

/// Production clock backed by the OS.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn local_offset_at(&self, at: DateTime<Utc>) -> FixedOffset {
        *at.with_timezone(&Local).offset()
    }
}

/// Deterministic test clock: the instant only changes when the test says so.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use chrono::{FixedOffset, TimeZone, Utc};
/// use tokenmon_core::{Clock, ManualClock};
///
/// let clock = ManualClock::new(Utc.with_ymd_and_hms(2026, 3, 1, 23, 30, 0).unwrap())
///     .with_offset(FixedOffset::west_opt(8 * 3600).unwrap());
/// clock.advance(Duration::from_secs(3600));
/// assert_eq!(clock.now_local().to_rfc3339(), "2026-03-01T16:30:00-08:00");
/// ```
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
    offset: FixedOffset,
}

/// Alias for tests that only need a clock that never moves.
pub type FixedClock = ManualClock;

impl ManualClock {
    /// Creates a clock at `now` with a UTC local offset.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(now),
            offset: FixedOffset::east_opt(0).expect("zero offset is valid"),
        }
    }

    /// Creates a clock at a Unix timestamp in seconds (UTC offset).
    ///
    /// # Panics
    /// If `secs` is outside chrono's representable range.
    pub fn from_unix(secs: i64) -> Self {
        Self::new(
            Utc.timestamp_opt(secs, 0)
                .single()
                .expect("unix timestamp in chrono range"),
        )
    }

    /// Returns the clock with a fixed local offset.
    #[must_use]
    pub fn with_offset(self, offset: FixedOffset) -> Self {
        Self { offset, ..self }
    }

    /// Moves the clock forward by `by`.
    pub fn advance(&self, by: Duration) {
        let delta = chrono::Duration::from_std(by).expect("duration fits chrono");
        let mut guard = self.now.lock().expect("manual clock lock");
        *guard += delta;
    }

    /// Jumps the clock to `to` (may go backwards, to test clock changes).
    pub fn set(&self, to: DateTime<Utc>) {
        *self.now.lock().expect("manual clock lock") = to;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("manual clock lock")
    }

    fn local_offset_at(&self, _at: DateTime<Utc>) -> FixedOffset {
        self.offset
    }
}
