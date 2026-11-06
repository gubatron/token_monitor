//! Startup behaviour that must not depend on the OS: what a failed startup
//! says, the bounded retry for the tray icon and the hand-off of a "show the
//! panel" request that arrives before the UI exists.
//!
//! The primary instance takes the single-instance lock first and everything
//! else after. If any later step fails the app must say so and exit, releasing
//! the lock, rather than sit invisible.

use std::sync::atomic::{AtomicBool, Ordering};

/// Title of the message box shown for a startup failure.
pub const FAILURE_TITLE: &str = "TokenMon could not start";

/// Why the primary instance gave up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupFailure {
    /// The notification-area icon could not be created (for example Explorer's
    /// taskbar was not ready after the retries).
    Tray(String),
    /// The single-instance pipe could not be created, so a second launch could
    /// never reach this instance.
    Pipe(String),
}

impl StartupFailure {
    /// The text for the message box and the log.
    ///
    /// ```
    /// use tokenmon_app::startup::StartupFailure;
    /// let text = StartupFailure::Tray("no taskbar".into()).message();
    /// assert!(text.contains("tray icon") && text.contains("no taskbar"));
    /// ```
    pub fn message(&self) -> String {
        match self {
            Self::Tray(e) => format!(
                "TokenMon could not create its tray icon ({e}).\n\nIf Windows Explorer is \
                 still starting, start TokenMon again in a moment. The log is in \
                 %LOCALAPPDATA%\\TokenMon\\logs."
            ),
            Self::Pipe(e) => format!(
                "TokenMon could not set up its single-instance channel ({e}).\n\nThe log is \
                 in %LOCALAPPDATA%\\TokenMon\\logs."
            ),
        }
    }
}

/// Runs `attempt` up to `attempts` times (at least once), calling
/// `before_retry(n)` with the number of failures so far between tries.
/// Returns the first success or the last error.
///
/// ```
/// use tokenmon_app::startup::retry;
/// let mut calls = 0;
/// let r: Result<u32, &str> = retry(3, |_| {}, || {
///     calls += 1;
///     if calls < 3 { Err("not yet") } else { Ok(calls) }
/// });
/// assert_eq!(r, Ok(3));
/// ```
///
/// # Errors
/// The last error once every attempt failed.
pub fn retry<T, E>(
    attempts: u32,
    mut before_retry: impl FnMut(u32),
    mut attempt: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut failures = 0;
    loop {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e) => {
                failures += 1;
                if failures >= attempts.max(1) {
                    return Err(e);
                }
                before_retry(failures);
            }
        }
    }
}

/// A "show the panel" request from a second launch, which may arrive on the
/// pipe thread before the UI thread is ready to act on it.
///
/// The request is remembered; whoever sees the UI become ready (or the UI
/// itself, once built) takes it exactly once.
#[derive(Debug, Default)]
pub struct PendingShow {
    pending: AtomicBool,
    ready: AtomicBool,
}

impl PendingShow {
    /// No request, UI not ready.
    pub const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            ready: AtomicBool::new(false),
        }
    }

    /// Records a request. Returns whether the UI is ready, i.e. whether the
    /// caller should post a task to it now (otherwise [`mark_ready`](Self::mark_ready)
    /// picks the request up).
    pub fn request(&self) -> bool {
        self.pending.store(true, Ordering::SeqCst);
        self.ready.load(Ordering::SeqCst)
    }

    /// The UI can take tasks. Returns whether a request is waiting (and
    /// consumes it).
    pub fn mark_ready(&self) -> bool {
        self.ready.store(true, Ordering::SeqCst);
        self.take()
    }

    /// Consumes a waiting request.
    pub fn take(&self) -> bool {
        self.pending.swap(false, Ordering::SeqCst)
    }
}
