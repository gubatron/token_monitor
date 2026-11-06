//! `ObservationLoop` for Rust: runs a handler with the latest value of a
//! tracked projection of a watched state (spec 01 section 8).
//!
//! The state is a [`tokio::sync::watch`] channel (the settings store has one).
//! A change that leaves the tracked value equal is not a dependency, so it
//! does not trigger the handler; a burst of changes inside the coalescing
//! window is handled once, with the latest value.

use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// A running observation. Dropping it cancels it.
#[derive(Debug)]
pub struct ObservationLoop {
    task: JoinHandle<()>,
}

impl ObservationLoop {
    /// Starts observing `state` on `runtime`.
    ///
    /// With `initial`, `handle` runs once with the current value before any
    /// change. Afterwards it runs after each change of `track`'s result,
    /// waiting `coalescing` first so a burst collapses into one call.
    pub fn start<S, V>(
        runtime: &Handle,
        mut state: watch::Receiver<S>,
        coalescing: Duration,
        initial: bool,
        track: impl Fn(&S) -> V + Send + 'static,
        mut handle: impl FnMut(V) + Send + 'static,
    ) -> Self
    where
        S: Send + Sync + 'static,
        V: PartialEq + Clone + Send + 'static,
    {
        // Read the first value here, not in the task: a change made right after
        // `start` returns must count as a change, not as the starting point.
        let mut last = track(&state.borrow_and_update());
        let task = runtime.spawn(async move {
            if initial {
                handle(last.clone());
            }
            while state.changed().await.is_ok() {
                if !coalescing.is_zero() {
                    tokio::time::sleep(coalescing).await;
                }
                let now = track(&state.borrow_and_update());
                if now != last {
                    last = now.clone();
                    handle(now);
                }
            }
        });
        Self { task }
    }

    /// Stops the observation; no handler call happens afterwards.
    pub fn cancel(&self) {
        self.task.abort();
    }
}

impl Drop for ObservationLoop {
    fn drop(&mut self) {
        self.task.abort();
    }
}
