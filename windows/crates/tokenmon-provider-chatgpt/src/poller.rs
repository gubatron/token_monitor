//! `ChatGptPoller` (spec 03 section 2.6): gating, the polling loop and the
//! publish step.
//!
//! The timing (interval, backoff, `Retry-After`, wake gate) is core's
//! [`PollingLoop`]; the fetch, stale-session and error step is core's
//! [`ProviderRefresh`]. The refresh closures captured by the loop hold only a
//! [`Weak`] reference to the poller, so dropping the poller ends the loop.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, ProviderRefresh};
use tokenmon_core::{
    AuthEvent, AuthEventListener, BoxFuture, Clock, ProviderAuthSession, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller, SessionGeneration,
};

use crate::client::Fetch;
use crate::token_cache::AccessTokenCache;
use crate::usage::ChatGptSnapshot;

/// Boxed fetch seam: takes the cookie header and session generation (the token
/// cache key), returns one [`Fetch`].
pub type FetchFn = dyn Fn(
        String,
        SessionGeneration,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Fetch, ProviderError>> + Send>>
    + Send
    + Sync;

/// Everything a [`ChatGptPoller`] is wired to.
pub struct ChatGptPollerConfig {
    /// The shared ChatGPT session.
    pub session: Arc<dyn ProviderAuthSession>,
    /// "Now".
    pub clock: Arc<dyn Clock>,
    /// The exchanged-token cache, cleared on sign-out and rejection.
    pub token_cache: Arc<AccessTokenCache>,
    /// The fetch seam; live: [`ChatGptPoller::live_fetch`].
    pub fetch: Arc<FetchFn>,
    /// `AppSettings.isProviderEnabled(.chatgpt)`.
    pub is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `AppSettings.pollInterval(for: .chatgpt, menuIsOpen:)`; `None` parks.
    pub poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
}

#[derive(Debug, Default)]
struct State {
    snapshot: Option<ChatGptSnapshot>,
    last_error: Option<ProviderError>,
    last_refreshed_at: Option<DateTime<Utc>>,
}

/// Clears the in-flight flag on drop.
struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct SessionEvents(Weak<ChatGptPoller>);

impl AuthEventListener for SessionEvents {
    fn on_auth_event(&self, _event: AuthEvent) {
        if let Some(poller) = self.0.upgrade() {
            poller.clear_snapshot();
        }
    }
}

/// Polls ChatGPT usage for the captured session cookie.
pub struct ChatGptPoller {
    auth: Arc<dyn ProviderAuthSession>,
    fetch: Arc<FetchFn>,
    token_cache: Arc<AccessTokenCache>,
    clock: Arc<dyn Clock>,
    is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    refreshing: AtomicBool,
    menu_open: AtomicBool,
    state: Mutex<State>,
    polling: Arc<PollingLoop>,
}

impl ChatGptPoller {
    /// Wires the poller to its session and builds (but does not start) its
    /// polling loop. Any session event (sign-out, account reset) drops the
    /// token, snapshot and error.
    ///
    /// # Panics
    /// Outside a tokio runtime (the loop spawns its waits on it).
    pub fn new(config: ChatGptPollerConfig) -> Arc<Self> {
        let poller = Arc::new_cyclic(|weak: &Weak<Self>| {
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
            .clock(Arc::clone(&config.clock))
            .build();
            Self {
                auth: config.session,
                fetch: config.fetch,
                token_cache: config.token_cache,
                clock: config.clock,
                is_enabled: config.is_enabled,
                poll_interval: config.poll_interval,
                refreshing: AtomicBool::new(false),
                menu_open: AtomicBool::new(false),
                state: Mutex::new(State::default()),
                polling,
            }
        });
        poller
            .auth
            .add_listener(Arc::new(SessionEvents(Arc::downgrade(&poller))));
        poller
    }

    /// The live fetch over any transport: one [`crate::ChatGptUsageClient`]
    /// per call, keyed by the session generation, sharing `token_cache`.
    pub fn live_fetch<T: crate::ChatGptTransport + 'static>(
        transport: Arc<T>,
        token_cache: Arc<AccessTokenCache>,
        clock: Arc<dyn Clock>,
    ) -> Arc<FetchFn> {
        Arc::new(move |cookie, generation| {
            let client = crate::ChatGptUsageClient::new(
                cookie,
                Arc::clone(&transport),
                Some(Arc::clone(&token_cache)),
                generation.0.to_string(),
            );
            let now = clock.now();
            Box::pin(async move { client.fetch_usage(now).await })
        })
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The polling loop (wake, pause and resume for the app shell).
    pub fn polling_loop(&self) -> &Arc<PollingLoop> {
        &self.polling
    }

    /// Whether a refresh is in flight.
    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::SeqCst)
    }

    /// Whether the menu is open.
    pub fn is_menu_open(&self) -> bool {
        self.menu_open.load(Ordering::SeqCst)
    }

    fn current_interval(&self) -> Option<Duration> {
        (self.poll_interval)(self.is_menu_open())
    }

    /// The displayed snapshot.
    pub fn snapshot(&self) -> Option<ChatGptSnapshot> {
        self.lock_state().snapshot.clone()
    }

    /// One refresh (the body of Swift `performRefresh`): gating, the guarded
    /// fetch and the publish step.
    ///
    /// Skips when the provider is disabled or a refresh is running. Without a
    /// cookie it flags `needs_sign_in` and skips.
    pub async fn perform_refresh(&self) -> PollOutcome {
        if !(self.is_enabled)() || self.refreshing.swap(true, Ordering::SeqCst) {
            return PollOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let Some(cookie) = self.auth.cookie_header().filter(|c| !c.is_empty()) else {
            self.auth.set_needs_sign_in(true);
            let mut state = self.lock_state();
            if state.snapshot.is_none() {
                state.last_error = Some(ProviderError::NotSignedIn(ProviderErrorContext::ChatGpt));
            }
            return PollOutcome::Skipped;
        };
        let generation = self.auth.state().session_generation;
        let failure: Mutex<Option<ProviderError>> = Mutex::new(None);
        ProviderRefresh::run(
            &*self.auth,
            "ChatGPT",
            || async {
                let result = (self.fetch)(cookie, generation).await;
                if let Err(error) = &result {
                    *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error.clone());
                }
                result
            },
            |fetched| self.publish(fetched),
            || self.token_cache.clear(),
            |_message| {
                self.lock_state().last_error = failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
            },
        )
        .await
    }

    fn publish(&self, fetch: Fetch) -> PollOutcome {
        self.auth.apply_refreshed_cookies(&fetch.set_cookie_headers);
        let snapshot = ChatGptSnapshot::new(fetch.response, fetch.fetched_at);
        tracing::info!(
            "ChatGPT refresh: {:.0}% used",
            snapshot.headline_used_percent()
        );
        {
            let mut state = self.lock_state();
            state.snapshot = Some(snapshot);
            state.last_error = None;
            state.last_refreshed_at = Some(self.clock.now());
        }
        self.auth.set_needs_sign_in(false);
        PollOutcome::Success
    }
}

impl ProviderUsagePoller for ChatGptPoller {
    fn provider(&self) -> ProviderId {
        ProviderId::ChatGpt
    }

    fn set_menu_open(&self, open: bool) {
        if self.menu_open.swap(open, Ordering::SeqCst) != open {
            self.polling.wake();
        }
    }

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        self.lock_state().last_refreshed_at
    }

    fn last_error(&self) -> Option<ProviderError> {
        self.lock_state().last_error.clone()
    }

    fn headline_used_percent(&self) -> Option<f64> {
        self.lock_state()
            .snapshot
            .as_ref()
            .map(ChatGptSnapshot::headline_used_percent)
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        self.lock_state()
            .snapshot
            .as_ref()
            .and_then(ChatGptSnapshot::resets_at)
    }

    fn alert_account(&self) -> Option<String> {
        self.auth.state().account_email
    }

    fn start(&self) {
        self.polling.start();
    }

    fn stop(&self) {
        self.polling.stop();
    }

    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.polling.refresh_now().await })
    }

    /// Sign-out / account reset: drops the token, snapshot and error.
    fn clear_snapshot(&self) {
        self.token_cache.clear();
        *self.lock_state() = State::default();
    }
}
