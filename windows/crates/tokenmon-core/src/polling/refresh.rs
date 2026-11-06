//! The fetch, stale-session and error steps every poller shares (spec 01
//! section 1.2.5).

use std::future::Future;

use super::{PollOutcome, RefreshError};
use crate::UsageError;
use crate::auth::AuthSession;

/// Namespace for the shared refresh step, [`ProviderRefresh::run`].
#[derive(Debug, Clone, Copy)]
pub struct ProviderRefresh;

impl ProviderRefresh {
    /// Runs `fetch` under the session generation current at the call.
    ///
    /// A value reaches `publish` only while that generation is still live, so a
    /// response that finishes after a sign-out or account switch is dropped
    /// ([`PollOutcome::Skipped`]). A failure is reported only for the live
    /// generation: a rejected session (unauthorized or not signed in) runs
    /// `on_rejected` and counts toward the session's rejection streak; every
    /// live failure goes to `report_error` and the log. Cancellation
    /// ([`RefreshError::Cancelled`]) is `Skipped`.
    ///
    /// `fetch` is a closure so the generation is read before any request is
    /// built. The error type may be a plain `ProviderError` or a
    /// [`RefreshError`].
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::auth::{AuthSession, AuthSessionState, SessionGeneration};
    /// use tokenmon_core::polling::{PollOutcome, ProviderRefresh};
    /// use tokenmon_core::{ProviderError, ProviderErrorContext};
    /// # use std::sync::Arc;
    /// # use tokenmon_core::AuthEventListener;
    /// # struct Live;
    /// # impl AuthSession for Live {
    /// #     fn cookie_header(&self) -> Option<String> { None }
    /// #     fn state(&self) -> AuthSessionState { AuthSessionState::default() }
    /// #     fn record_auth_failure(&self, _: &str) -> bool { false }
    /// #     fn record_auth_success(&self) {}
    /// #     fn is_current(&self, g: SessionGeneration) -> bool { g == SessionGeneration(0) }
    /// #     fn add_listener(&self, _: Arc<dyn AuthEventListener>) {}
    /// # }
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let mut published = Vec::new();
    /// let outcome = ProviderRefresh::run(
    ///     &Live,
    ///     "Claude",
    ///     || async { Ok::<_, ProviderError>(7) },
    ///     |v| { published.push(v); PollOutcome::Success },
    ///     || {},
    ///     |_message| {},
    /// )
    /// .await;
    /// assert_eq!(outcome, PollOutcome::Success);
    /// assert_eq!(published, [7]);
    /// # });
    /// ```
    ///
    /// # Returns
    /// The outcome for `PollingLoop` backoff.
    pub async fn run<A, V, E, Fut>(
        auth: &A,
        label: &str,
        fetch: impl FnOnce() -> Fut,
        publish: impl FnOnce(V) -> PollOutcome,
        on_rejected: impl FnOnce(),
        report_error: impl FnOnce(String),
    ) -> PollOutcome
    where
        A: AuthSession + ?Sized,
        Fut: Future<Output = Result<V, E>>,
        E: Into<RefreshError>,
    {
        let generation = auth.state().session_generation;
        match fetch().await {
            Ok(value) => {
                if !auth.is_current(generation) {
                    return PollOutcome::Skipped;
                }
                auth.record_auth_success();
                publish(value)
            }
            Err(error) => match error.into() {
                RefreshError::Cancelled => PollOutcome::Skipped,
                RefreshError::Provider(error) => {
                    if !auth.is_current(generation) {
                        return PollOutcome::Skipped;
                    }
                    let message = error.to_string();
                    if matches!(
                        error.usage_error(),
                        UsageError::Unauthorized | UsageError::NotSignedIn
                    ) {
                        on_rejected();
                        auth.record_auth_failure(&message);
                    }
                    tracing::error!("{label} refresh failed: {message}");
                    report_error(message);
                    PollOutcome::from(&error)
                }
            },
        }
    }
}
