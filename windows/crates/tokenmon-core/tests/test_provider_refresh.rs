//! `ProviderRefresh::run` (port of ProviderRefreshTests).
#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokenmon_core::auth::{AuthEventListener, AuthSession, AuthSessionState, SessionGeneration};
use tokenmon_core::polling::{PollOutcome, ProviderRefresh, RefreshError};
use tokenmon_core::{ProviderError, ProviderErrorContext};

struct FakeAuth {
    generation: AtomicU64,
    live: AtomicBool,
    failures: Mutex<Vec<String>>,
    successes: AtomicUsize,
}

impl FakeAuth {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            live: AtomicBool::new(true),
            failures: Mutex::default(),
            successes: AtomicUsize::new(0),
        }
    }
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl AuthSession for FakeAuth {
    fn cookie_header(&self) -> Option<String> {
        None
    }
    fn state(&self) -> AuthSessionState {
        AuthSessionState {
            session_generation: SessionGeneration(self.generation.load(Ordering::SeqCst)),
            ..AuthSessionState::default()
        }
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        self.failures.lock().unwrap().push(reason.to_owned());
        false
    }
    fn record_auth_success(&self) {
        self.successes.fetch_add(1, Ordering::SeqCst);
    }
    fn is_current(&self, g: SessionGeneration) -> bool {
        self.live.load(Ordering::SeqCst) && g.0 == self.generation.load(Ordering::SeqCst)
    }
    fn add_listener(&self, _: Arc<dyn AuthEventListener>) {}
}

#[derive(Default)]
struct Sinks {
    published: Vec<i32>,
    rejected: usize,
    errors: Vec<String>,
}

async fn run<E: Into<RefreshError>>(
    auth: &FakeAuth,
    fetch: impl FnOnce() -> Result<i32, E>,
) -> (PollOutcome, Sinks) {
    let sinks = Mutex::new(Sinks::default());
    let outcome = ProviderRefresh::run(
        auth,
        "Test",
        || async { fetch() },
        |v| {
            sinks.lock().unwrap().published.push(v);
            PollOutcome::Success
        },
        || sinks.lock().unwrap().rejected += 1,
        |m| sinks.lock().unwrap().errors.push(m),
    )
    .await;
    (outcome, sinks.into_inner().unwrap())
}

fn unauthorized() -> ProviderError {
    ProviderError::Unauthorized(ProviderErrorContext::Claude)
}

/// Success publishes and resets the streak.
#[tokio::test]
async fn test_success_publishes_and_resets_the_streak() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || Ok::<_, ProviderError>(7)).await;
    assert_eq!(outcome, PollOutcome::Success);
    assert_eq!(s.published, [7]);
    assert_eq!(auth.successes.load(Ordering::SeqCst), 1);
    assert!(s.errors.is_empty());
}

/// A generation bumped during the fetch gives skipped; nothing published.
#[tokio::test]
async fn test_result_for_a_stale_generation_is_dropped() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || {
        auth.bump();
        Ok::<_, ProviderError>(7)
    })
    .await;
    assert_eq!(outcome, PollOutcome::Skipped);
    assert!(s.published.is_empty());
    assert_eq!(auth.successes.load(Ordering::SeqCst), 0);
}

/// A signed-out (not live) session also drops the result.
#[tokio::test]
async fn test_result_for_a_signed_out_session_is_dropped() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || {
        auth.live.store(false, Ordering::SeqCst);
        Ok::<_, ProviderError>(7)
    })
    .await;
    assert_eq!(outcome, PollOutcome::Skipped);
    assert!(s.published.is_empty());
}

/// Unauthorized gives failure(None), one rejection, one streak hit, one error.
#[tokio::test]
async fn test_rejected_session_counts_toward_the_streak() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || Err(unauthorized())).await;
    assert_eq!(outcome, PollOutcome::Failure { retry_after: None });
    assert_eq!(s.rejected, 1);
    assert_eq!(auth.failures.lock().unwrap().len(), 1);
    assert_eq!(s.errors.len(), 1);
}

/// NotSignedIn is a rejection too.
#[tokio::test]
async fn test_not_signed_in_counts_toward_the_streak() {
    let auth = FakeAuth::new();
    let (_, s) = run(&auth, || {
        Err(ProviderError::NotSignedIn(ProviderErrorContext::Cursor))
    })
    .await;
    assert_eq!(s.rejected, 1);
    assert_eq!(auth.failures.lock().unwrap().len(), 1);
}

/// A network failure is a failure but not a rejection.
#[tokio::test]
async fn test_other_failures_do_not_count_against_the_session() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || {
        Err(ProviderError::Network(
            ProviderErrorContext::Claude,
            "offline".into(),
        ))
    })
    .await;
    assert_eq!(outcome, PollOutcome::Failure { retry_after: None });
    assert_eq!(s.rejected, 0);
    assert!(auth.failures.lock().unwrap().is_empty());
    assert_eq!(s.errors.len(), 1);
}

/// A failure for a stale generation is ignored: skipped, no streak, no error.
#[tokio::test]
async fn test_failure_for_a_stale_generation_is_ignored() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || {
        auth.bump();
        Err(unauthorized())
    })
    .await;
    assert_eq!(outcome, PollOutcome::Skipped);
    assert!(auth.failures.lock().unwrap().is_empty());
    assert!(s.errors.is_empty());
    assert_eq!(s.rejected, 0);
}

/// Cancellation is skipped with no error.
#[tokio::test]
async fn test_cancellation_is_skipped() {
    let auth = FakeAuth::new();
    let (outcome, s) = run(&auth, || Err::<i32, _>(RefreshError::Cancelled)).await;
    assert_eq!(outcome, PollOutcome::Skipped);
    assert!(s.errors.is_empty());
}

/// A rate-limit failure carries its Retry-After into the outcome.
#[tokio::test]
async fn test_rate_limit_failure_carries_retry_after() {
    use std::time::Duration;
    use tokenmon_core::UsageError;
    let auth = FakeAuth::new();
    let (outcome, _) = run(&auth, || {
        Err(ProviderError::from_usage(
            UsageError::RateLimited {
                retry_after: Some(42.0),
            },
            ProviderErrorContext::Claude,
        ))
    })
    .await;
    assert_eq!(
        outcome,
        PollOutcome::Failure {
            retry_after: Some(Duration::from_secs(42))
        }
    );
}
