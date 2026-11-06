//! `OpenRouterUsagePoller` (spec 03 section 4.5): the refresh state machine.
//!
//! This type owns what one refresh does and the published state, drives
//! itself on core's [`PollingLoop`] and implements [`ProviderUsagePoller`] so
//! the registry can hold it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, ProviderRefresh};
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, BoxFuture, Clock, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};

use crate::auth::{ApiKey, OpenRouterAuthSession};
use crate::client::OpenRouterUsageClient;
use crate::snapshot::OpenRouterSnapshot;

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
    m.lock().unwrap_or_else(PoisonError::into_inner)
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
///
/// The timing (interval, backoff, `Retry-After`, wake gate) is core's
/// [`PollingLoop`]; the stale-session and error step is core's
/// [`ProviderRefresh`]. The loop's closures hold only a [`Weak`] reference, so
/// dropping the poller ends the loop.
pub struct OpenRouterUsagePoller {
    auth: Arc<OpenRouterAuthSession>,
    clock: Arc<dyn Clock>,
    factory: ClientFactory,
    is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    state: Arc<Mutex<State>>,
    refreshing: AtomicBool,
    menu_open: AtomicBool,
    polling: Arc<PollingLoop>,
}

impl OpenRouterUsagePoller {
    /// Wires the poller to its session, registers the sign-out listener and
    /// builds (but does not start) its polling loop.
    ///
    /// `is_enabled` is `AppSettings.isProviderEnabled(.openRouter)` and
    /// `poll_interval` is `AppSettings.pollInterval(for:menuIsOpen:)`
    /// (`None` parks the loop).
    ///
    /// # Panics
    /// Outside a tokio runtime (the loop spawns its waits on it).
    pub fn new(
        auth: Arc<OpenRouterAuthSession>,
        clock: Arc<dyn Clock>,
        factory: ClientFactory,
        is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    ) -> Arc<Self> {
        let state = Arc::new(Mutex::new(State::default()));
        auth.add_listener(Arc::new(SignedOutClears(Arc::clone(&state))));
        Arc::new_cyclic(|weak: &Weak<Self>| {
            let for_interval = weak.clone();
            let for_refresh = weak.clone();
            let polling = PollingLoop::builder(
                move || for_interval.upgrade().and_then(|p| p.current_interval()),
                move || {
                    let weak = for_refresh.clone();
                    async move {
                        match weak.upgrade() {
                            Some(poller) => poller.perform_refresh().await,
                            None => PollOutcome::Skipped,
                        }
                    }
                },
            )
            .clock(Arc::clone(&clock))
            .build();
            Self {
                auth,
                clock,
                factory,
                is_enabled,
                poll_interval,
                state,
                refreshing: AtomicBool::new(false),
                menu_open: AtomicBool::new(false),
                polling,
            }
        })
    }

    /// The polling loop (wake, pause and resume for the app shell).
    pub fn polling_loop(&self) -> &Arc<PollingLoop> {
        &self.polling
    }

    /// The latest snapshot.
    pub fn snapshot(&self) -> Option<OpenRouterSnapshot> {
        lock(&self.state).snapshot.clone()
    }

    /// Whether the menu is open (selects the poll interval).
    pub fn is_menu_open(&self) -> bool {
        self.menu_open.load(Ordering::SeqCst)
    }

    fn current_interval(&self) -> Option<Duration> {
        (self.poll_interval)(self.is_menu_open())
    }

    /// One refresh attempt.
    ///
    /// Skips when the provider is disabled, a refresh is running, there is no
    /// key (flags `needs_sign_in`) or the key was already rejected.
    pub async fn perform_refresh(&self) -> PollOutcome {
        if !(self.is_enabled)() || self.refreshing.swap(true, Ordering::SeqCst) {
            return PollOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let Some(key) = self.auth.api_key() else {
            self.auth.set_needs_sign_in(true);
            let mut s = lock(&self.state);
            if s.snapshot.is_none() {
                s.last_error = Some(ProviderError::NotSignedIn(ProviderErrorContext::OpenRouter));
            }
            return PollOutcome::Skipped;
        };
        let session = self.auth.state();
        if !session.is_signed_in || session.needs_sign_in {
            return PollOutcome::Skipped;
        }
        let failure: Mutex<Option<ProviderError>> = Mutex::new(None);
        ProviderRefresh::run(
            &*self.auth,
            "OpenRouter",
            || async {
                let result = match (self.factory)(&key) {
                    Ok(client) => client.fetch_snapshot(self.clock.now()).await,
                    Err(e) => Err(e),
                };
                if let Err(error) = &result {
                    *lock(&failure) = Some(error.clone());
                }
                result
            },
            |snapshot| self.publish(snapshot),
            || {},
            |_message| lock(&self.state).last_error = lock(&failure).take(),
        )
        .await
    }

    fn publish(&self, snapshot: OpenRouterSnapshot) -> PollOutcome {
        self.auth.mark_fetch_succeeded();
        let mut s = lock(&self.state);
        s.snapshot = Some(snapshot);
        s.last_error = None;
        s.last_refreshed_at = Some(self.clock.now());
        PollOutcome::Success
    }
}

impl ProviderUsagePoller for OpenRouterUsagePoller {
    fn provider(&self) -> ProviderId {
        ProviderId::OpenRouter
    }

    fn set_menu_open(&self, open: bool) {
        if self.menu_open.swap(open, Ordering::SeqCst) != open {
            self.polling.wake();
        }
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
        self.polling.start();
    }

    fn stop(&self) {
        self.polling.stop();
    }

    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.polling.refresh_now().await;
        })
    }

    fn clear_snapshot(&self) {
        lock(&self.state).clear();
    }
}
