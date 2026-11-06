//! Polling: [`BackoffTimer`], [`PollingLoop`], [`PollOutcome`] and
//! [`ProviderRefresh`] (spec 01 section 1.2).
//!
//! Owner: ws-core-polling. The [`Clock`](crate::Clock) trait itself lives in
//! [`crate::clock`].
//!
//! # Shape
//!
//! Each provider poller owns one [`PollingLoop`]. The loop asks an `interval`
//! closure how long to wait (`None` parks it with no timer at all), runs the
//! `refresh` closure, and feeds the returned [`PollOutcome`] into a
//! [`BackoffTimer`] and a `Retry-After` deadline. Inputs that change the
//! interval (menu opened, settings edited) call [`PollingLoop::wake`]; system
//! sleep calls [`PollingLoop::pause`] and [`PollingLoop::resume`] (the "wake
//! gate").
//!
//! [`ProviderRefresh::run`] is the fetch / stale-session / error step every
//! provider shares, producing the [`PollOutcome`].
//!
//! # Testing
//!
//! All time is injected: [`Clock`](crate::Clock) for "now", [`Sleeper`] for the
//! wait and [`JitterSource`] for jitter. [`testing::ManualScheduler`] provides a
//! deterministic clock plus sleeper for tests in any crate.
//!
//! # Examples
//!
//! ```
//! use std::time::Duration;
//! use tokenmon_core::polling::{PollOutcome, PollingLoop};
//!
//! # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
//! let poller = PollingLoop::builder(
//!     || Some(Duration::from_secs(300)),
//!     || async { PollOutcome::Success },
//! )
//! .build();
//! poller.start(); // refreshes now, then every 300 s (+/- 10% jitter)
//! poller.stop();
//! # });
//! ```

mod backoff;
mod outcome;
mod polling_loop;
mod refresh;
mod time;

pub mod testing;

pub use backoff::BackoffTimer;
pub use outcome::{PollOutcome, RefreshError};
pub use polling_loop::{PollingLoop, PollingLoopBuilder};
pub use refresh::ProviderRefresh;
pub use time::{JitterSource, RandomJitter, Sleeper, TokioSleeper};
