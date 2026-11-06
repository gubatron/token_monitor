//! Injectable waiting and jitter for the polling loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::provider::BoxFuture;

/// Longest single wait the real sleeper performs (one day).
const MAX_SLEEP: Duration = Duration::from_hours(24);

/// Suspends for a duration. Cancellation is by dropping the returned future.
///
/// Tests inject [`ManualScheduler`](super::testing::ManualScheduler); production
/// uses [`TokioSleeper`].
pub trait Sleeper: Send + Sync {
    /// A future that completes after `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

/// Real sleeper on the tokio timer, clamped to one day. Honours tokio's paused
/// time in tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSleeper;

impl Sleeper for TokioSleeper {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration.min(MAX_SLEEP)))
    }
}

/// Source of the jitter fraction added to each wait.
///
/// The loop clamps whatever is returned to `-0.1..=0.1`. Any
/// `Fn() -> f64 + Send + Sync` closure is a source, which is how tests inject a
/// fixed value.
pub trait JitterSource: Send + Sync {
    /// The next fraction, nominally uniform in `-0.1..=0.1`.
    fn next_fraction(&self) -> f64;
}

impl<F> JitterSource for F
where
    F: Fn() -> f64 + Send + Sync,
{
    fn next_fraction(&self) -> f64 {
        self()
    }
}

/// Default jitter: a small splitmix64 generator seeded from the system clock.
///
/// Not cryptographic; it only needs to drift loops that started together apart.
#[derive(Debug)]
pub struct RandomJitter {
    state: AtomicU64,
}

impl RandomJitter {
    /// Creates a generator with an explicit seed (deterministic).
    pub const fn with_seed(seed: u64) -> Self {
        Self {
            state: AtomicU64::new(seed),
        }
    }

    /// Creates a generator seeded from the current time.
    pub fn from_entropy() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        Self::with_seed(nanos)
    }

    fn next_u64(&self) -> u64 {
        let mut z = self
            .state
            .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Default for RandomJitter {
    fn default() -> Self {
        Self::from_entropy()
    }
}

impl JitterSource for RandomJitter {
    /// Uniform in `-0.1..0.1`.
    fn next_fraction(&self) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        (unit * 2.0 - 1.0) * 0.1
    }
}
