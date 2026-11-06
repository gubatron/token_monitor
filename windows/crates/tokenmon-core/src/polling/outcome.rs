//! What one refresh achieved (spec 01 section 1.2.2).

use std::time::Duration;

use crate::ProviderError;

/// What one refresh achieved, so the loop can back off after failures.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use tokenmon_core::polling::{PollOutcome, RefreshError};
/// use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};
///
/// let limited = ProviderError::from_usage(
///     UsageError::RateLimited { retry_after: Some(42.0) },
///     ProviderErrorContext::Claude,
/// );
/// assert_eq!(
///     PollOutcome::from(&limited),
///     PollOutcome::Failure { retry_after: Some(Duration::from_secs(42)) }
/// );
/// assert_eq!(PollOutcome::from(&RefreshError::Cancelled), PollOutcome::Skipped);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollOutcome {
    /// Fresh data arrived; any backoff is cleared.
    Success,
    /// The fetch failed. `retry_after` is the server's requested wait, if known.
    Failure {
        /// The server's `Retry-After`, honoured without jitter.
        retry_after: Option<Duration>,
    },
    /// Nothing was fetched (not needed, signed out, stale session, cancelled).
    Skipped,
}

/// Why a fetch did not produce a value, as seen by `ProviderRefresh::run`.
///
/// Rust has no implicit task cancellation error, so cooperative cancellation
/// is explicit. A plain [`ProviderError`] converts with `.into()`.
#[derive(Debug, Clone, PartialEq)]
pub enum RefreshError {
    /// The fetch was cancelled; reported as [`PollOutcome::Skipped`].
    Cancelled,
    /// The provider failed.
    Provider(ProviderError),
}

impl From<ProviderError> for RefreshError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

impl From<&ProviderError> for PollOutcome {
    /// A failure carrying the error's `Retry-After`, if any.
    fn from(error: &ProviderError) -> Self {
        Self::Failure {
            retry_after: error.usage_error().retry_after(),
        }
    }
}

impl From<&RefreshError> for PollOutcome {
    /// Cancellation is [`Skipped`](Self::Skipped); anything else is a failure.
    fn from(error: &RefreshError) -> Self {
        match error {
            RefreshError::Cancelled => Self::Skipped,
            RefreshError::Provider(e) => Self::from(e),
        }
    }
}
