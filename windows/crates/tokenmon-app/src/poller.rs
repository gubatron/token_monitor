//! The poller contract the app drives: [`ProviderUsagePoller`] plus the loop
//! controls the lifecycle needs.

use std::sync::Arc;
use std::time::Duration;

use tokenmon_core::{BoxFuture, ProviderUsagePoller};

/// A provider poller whose timed loop the app can wake, pause and resume.
///
/// The core [`ProviderUsagePoller`] contract covers state and `start`/`stop`;
/// the lifecycle also needs the loop controls (system sleep pauses every loop,
/// opening the panel refreshes anything older than 15 s, a settings change
/// re-evaluates every interval). Real pollers delegate to their
/// `PollingLoop`; tests use [`crate::testing::FakePoller`].
pub trait ManagedPoller: ProviderUsagePoller {
    /// Re-evaluates the polling interval now (settings or menu state changed).
    fn wake(&self);
    /// Suspends the timer (system sleep) without forgetting the last refresh.
    fn pause(&self);
    /// Resumes after [`pause`](Self::pause); an overdue loop refreshes at once.
    fn resume(&self);
    /// Refreshes only when the last refresh is older than `age`.
    fn refresh_now_if_older_than(&self, age: Duration) -> BoxFuture<'_, ()>;
}

/// Implements [`ManagedPoller`] for a poller type exposing `polling_loop()`.
macro_rules! managed_via_loop {
    ($($poller:ty),+ $(,)?) => {$(
        impl ManagedPoller for $poller {
            fn wake(&self) {
                self.polling_loop().wake();
            }
            fn pause(&self) {
                self.polling_loop().pause();
            }
            fn resume(&self) {
                self.polling_loop().resume();
            }
            fn refresh_now_if_older_than(&self, age: Duration) -> BoxFuture<'_, ()> {
                Box::pin(async move { self.polling_loop().refresh_now_if_older_than(age).await })
            }
        }
    )+};
}

managed_via_loop!(
    tokenmon_provider_grok::provider_poller::GrokProviderPoller,
    tokenmon_provider_cursor::poller::CursorUsagePoller,
    tokenmon_provider_claude::poller::ClaudeUsagePoller,
    tokenmon_provider_opencode::poller::OpenCodeUsagePoller,
    tokenmon_provider_chatgpt::ChatGptPoller,
    tokenmon_provider_grokbot::GrokbotUsagePoller,
    tokenmon_provider_openrouter::OpenRouterUsagePoller,
);

/// Shared handle to a managed poller.
pub type SharedPoller = Arc<dyn ManagedPoller>;
