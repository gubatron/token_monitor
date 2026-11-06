//! Deterministic time for polling tests, usable from any crate.
//!
//! [`ManualScheduler`] is both a [`Sleeper`] and the owner of a [`ManualClock`].
//! Sleeps never complete on their own: the test inspects them with
//! [`next_sleep`](ManualScheduler::next_sleep) and completes them with
//! [`fire`](ManualScheduler::fire), which also advances the clock by the slept
//! duration, exactly as real time would.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use std::time::Duration;
//! use tokenmon_core::polling::testing::ManualScheduler;
//! use tokenmon_core::polling::{PollOutcome, PollingLoop};
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
//! let time = ManualScheduler::new();
//! let poller = PollingLoop::builder(|| Some(Duration::from_secs(60)), || async {
//!     PollOutcome::Success
//! })
//! .clock(time.clock())
//! .sleeper(Arc::new(time.clone()))
//! .jitter(Arc::new(|| 0.0))
//! .build();
//! poller.start();
//! assert_eq!(time.next_sleep().await, Duration::from_secs(60));
//! poller.stop();
//! # });
//! ```

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokio::sync::{Notify, oneshot};

use super::Sleeper;
use crate::clock::ManualClock;
use crate::provider::BoxFuture;

struct Pending {
    id: u64,
    duration: Duration,
    complete: oneshot::Sender<()>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    /// Sleeps started but not yet observed by `next_sleep`.
    unobserved: VecDeque<Duration>,
    pending: Vec<Pending>,
    requested: Vec<Duration>,
    cancelled: usize,
}

struct Shared {
    clock: Arc<ManualClock>,
    inner: Mutex<Inner>,
    started: Notify,
}

/// Removes a sleep from `pending` when its future is dropped, counting it as
/// cancelled if it had not been fired.
struct SleepGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        let mut inner = self.shared.lock();
        if let Some(pos) = inner.pending.iter().position(|p| p.id == self.id) {
            inner.pending.remove(pos);
            inner.cancelled += 1;
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A manual clock plus a sleeper whose sleeps the test completes by hand.
#[derive(Clone)]
pub struct ManualScheduler {
    shared: Arc<Shared>,
}

impl Default for ManualScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualScheduler {
    /// Creates a scheduler at 2026-01-01T00:00:00Z.
    pub fn new() -> Self {
        let start = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .expect("valid start instant");
        Self {
            shared: Arc::new(Shared {
                clock: Arc::new(ManualClock::new(start)),
                inner: Mutex::new(Inner::default()),
                started: Notify::new(),
            }),
        }
    }

    /// The shared clock, to pass to `PollingLoopBuilder::clock`.
    pub fn clock(&self) -> Arc<ManualClock> {
        Arc::clone(&self.shared.clock)
    }

    /// Moves the clock forward without completing any sleep.
    pub fn advance(&self, by: Duration) {
        self.shared.clock.advance(by);
    }

    /// Waits for the next sleep the loop starts and returns its duration.
    pub async fn next_sleep(&self) -> Duration {
        loop {
            if let Some(d) = self.shared.lock().unobserved.pop_front() {
                return d;
            }
            self.shared.started.notified().await;
        }
    }

    /// Lets every ready task run: yields to the runtime repeatedly.
    pub async fn settle(&self) {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    /// Completes every pending sleep, advancing the clock by each duration.
    pub fn fire(&self) {
        let fired: Vec<Pending> = std::mem::take(&mut self.shared.lock().pending);
        for p in fired {
            self.shared.clock.advance(p.duration);
            // The receiver may already be gone if the wait was cancelled.
            let _ = p.complete.send(());
        }
    }

    /// Sleeps started and neither fired nor cancelled.
    pub fn pending_count(&self) -> usize {
        self.shared.lock().pending.len()
    }

    /// Sleeps dropped before they were fired.
    pub fn cancelled_count(&self) -> usize {
        self.shared.lock().cancelled
    }

    /// Durations of every sleep requested so far, in order.
    pub fn requested(&self) -> Vec<Duration> {
        self.shared.lock().requested.clone()
    }
}

impl Sleeper for ManualScheduler {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = self.shared.lock();
            let id = inner.next_id;
            inner.next_id += 1;
            inner.pending.push(Pending {
                id,
                duration,
                complete: tx,
            });
            inner.unobserved.push_back(duration);
            inner.requested.push(duration);
            id
        };
        self.shared.started.notify_one();
        let guard = SleepGuard {
            shared: Arc::clone(&self.shared),
            id,
        };
        Box::pin(async move {
            let _guard = guard;
            let _ = rx.await;
        })
    }
}
