//! `OpenRouterUsagePoller` (spec 03 section 4.5): the refresh state machine.
//!
//! The timed loop belongs to the core polling module and the app; this type
//! owns what one refresh does and the published state, and implements
//! [`ProviderUsagePoller`] so the registry can hold it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, BoxFuture, Clock, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};

use crate::auth::{ApiKey, OpenRouterAuthSession};
use crate::client::OpenRouterUsageClient;
use crate::snapshot::OpenRouterSnapshot;

/// Result of one refresh attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Disabled, already refreshing, no key, or key rejected.
    Skipped,
    /// A snapshot was published.
    Success,
    /// The fetch failed; the error is in `last_error`.
    Failure,
}

/// Builds a usage client for a key (live: reqwest; tests: fakes).
pub type ClientFactory =
    Arc<dyn Fn(&ApiKey) -> Result<OpenRouterUsageClient, ProviderError> + Send + Sync>;

#[derive(Default)]
struct State {
    snapshot: Option<OpenRouterSnapshot>,
    last_error: Option<ProviderError>,
    last_refreshed_at: Option<DateTime<Utc>>,
}

impl State {
    fn clear(&mut self) {
        *self = Self::default();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct SignedOutClears(Arc<Mutex<State>>);

impl AuthEventListener for SignedOutClears {
    fn on_auth_event(&self, event: AuthEvent) {
        if event == AuthEvent::SignedOut {
            lock(&self.0).clear();
        }
    }
}

struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Polls OpenRouter usage for the saved API key.
pub struct OpenRouterUsagePoller {
    auth: Arc<OpenRouterAuthSession>,
    clock: Arc<dyn Clock>,
    factory: ClientFactory,
    is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    state: Arc<Mutex<State>>,
    refreshing: AtomicBool,
    menu_open: AtomicBool,
    running: AtomicBool,
}

impl OpenRouterUsagePoller {
    /// Wires the poller to its session and registers the sign-out listener.
    pub fn new(
        auth: Arc<OpenRouterAuthSession>,
        clock: Arc<dyn Clock>,
        factory: ClientFactory,
        is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        auth.add_listener(Arc::new(SignedOutClears(Arc::clone(&state))));
        Self {
            auth,
            clock,
            factory,
            is_enabled,
            state,
            refreshing: AtomicBool::new(false),
            menu_open: AtomicBool::new(false),
            running: AtomicBool::new(false),
        }
    }

    /// The latest snapshot.
    pub fn snapshot(&self) -> Option<OpenRouterSnapshot> {
        lock(&self.state).snapshot.clone()
    }

    /// Whether the app asked this poller to run.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Whether the menu is open (selects the poll interval upstream).
    pub fn is_menu_open(&self) -> bool {
        self.menu_open.load(Ordering::SeqCst)
    }

    /// One refresh attempt.
    pub async fn refresh(&self) -> RefreshOutcome {
        if !(self.is_enabled)() || self.refreshing.swap(true, Ordering::SeqCst) {
            return RefreshOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let Some(key) = self.auth.api_key() else {
            self.auth.set_needs_sign_in(true);
            let mut s = lock(&self.state);
            if s.snapshot.is_none() {
                s.last_error = Some(ProviderError::NotSignedIn(ProviderErrorContext::OpenRouter));
            }
            return RefreshOutcome::Skipped;
        };
        let session = self.auth.state();
        if !session.is_signed_in || session.needs_sign_in {
            return RefreshOutcome::Skipped;
        }
        self.fetch_and_publish(&key, session.session_generation)
            .await
    }

    async fn fetch_and_publish(
        &self,
        key: &ApiKey,
        generation: tokenmon_core::SessionGeneration,
    ) -> RefreshOutcome {
        let result = match (self.factory)(key) {
            Ok(client) => client.fetch_snapshot(self.clock.now()).await,
            Err(e) => Err(e),
        };
        if !self.auth.is_current(generation) {
            return RefreshOutcome::Skipped;
        }
        match result {
            Ok(snapshot) => {
                self.auth.record_auth_success();
                self.auth.mark_fetch_succeeded();
                let mut s = lock(&self.state);
                s.snapshot = Some(snapshot);
                s.last_error = None;
                s.last_refreshed_at = Some(self.clock.now());
                RefreshOutcome::Success
            }
            Err(e) => {
                if matches!(e, ProviderError::Unauthorized(_)) {
                    self.auth.record_auth_failure(&e.to_string());
                }
                tracing::warn!("OpenRouter refresh failed: {e}");
                lock(&self.state).last_error = Some(e);
                RefreshOutcome::Failure
            }
        }
    }
}

impl ProviderUsagePoller for OpenRouterUsagePoller {
    fn provider(&self) -> ProviderId {
        ProviderId::OpenRouter
    }

    fn set_menu_open(&self, open: bool) {
        self.menu_open.store(open, Ordering::SeqCst);
    }

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        lock(&self.state).last_refreshed_at
    }

    fn last_error(&self) -> Option<ProviderError> {
        lock(&self.state).last_error.clone()
    }

    fn headline_used_percent(&self) -> Option<f64> {
        lock(&self.state)
            .snapshot
            .as_ref()
            .and_then(OpenRouterSnapshot::used_percent)
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        None
    }

    fn alert_account(&self) -> Option<String> {
        None
    }

    fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
    }

    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.refresh().await;
        })
    }

    fn clear_snapshot(&self) {
        lock(&self.state).clear();
    }
}
