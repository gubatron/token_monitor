//! ChatGPT poller state machine (spec 03 section 2.6 and the shared
//! `ProviderRefresh.run` of spec 01 section 1.2.5).
//!
//! The polling loop (interval, backoff, wake) is owned by `ws-core-polling`;
//! this type is the per-refresh logic it will drive via [`ChatGptPoller::refresh_now`].

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use tokenmon_core::{
    AuthEvent, AuthEventListener, Clock, ProviderAuthSession, ProviderError, ProviderErrorContext,
    SessionGeneration, UsageError,
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

/// Result of one refresh attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum RefreshOutcome {
    /// A snapshot was published.
    Success,
    /// Nothing happened (disabled, already refreshing, no cookie, stale session).
    Skipped,
    /// The fetch failed; the error is also in `last_error`.
    Failed(ProviderError),
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

/// Polls ChatGPT usage for the captured session cookie.
pub struct ChatGptPoller {
    auth: Arc<dyn ProviderAuthSession>,
    fetch: Arc<FetchFn>,
    token_cache: Arc<AccessTokenCache>,
    clock: Arc<dyn Clock>,
    enabled: AtomicBool,
    refreshing: AtomicBool,
    state: Mutex<State>,
}

impl ChatGptPoller {
    /// `fetch` is the injected seam; build the live one with
    /// [`ChatGptPoller::live_fetch`]. The poller starts enabled.
    pub fn new(
        auth: Arc<dyn ProviderAuthSession>,
        clock: Arc<dyn Clock>,
        token_cache: Arc<AccessTokenCache>,
        fetch: Arc<FetchFn>,
    ) -> Self {
        Self {
            auth,
            fetch,
            token_cache,
            clock,
            enabled: AtomicBool::new(true),
            refreshing: AtomicBool::new(false),
            state: Mutex::new(State::default()),
        }
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

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `settings.isProviderEnabled(.chatgpt)`.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// The displayed snapshot.
    pub fn snapshot(&self) -> Option<ChatGptSnapshot> {
        self.state().snapshot.clone()
    }

    /// The last refresh error, cleared by a success.
    pub fn last_error(&self) -> Option<ProviderError> {
        self.state().last_error.clone()
    }

    /// When the last successful publish happened.
    pub fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        self.state().last_refreshed_at
    }

    /// `snapshot?.headlineUsedPercent`.
    pub fn headline_used_percent(&self) -> Option<f64> {
        self.state()
            .snapshot
            .as_ref()
            .map(ChatGptSnapshot::headline_used_percent)
    }

    /// `snapshot?.resetsAt`.
    pub fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        self.state()
            .snapshot
            .as_ref()
            .and_then(ChatGptSnapshot::resets_at)
    }

    /// `auth.accountEmail`.
    pub fn alert_account(&self) -> Option<String> {
        self.auth.state().account_email
    }

    /// Sign-out / account reset: drops the token, snapshot and error.
    pub fn clear_snapshot(&self) {
        self.token_cache.clear();
        *self.state() = State::default();
    }

    /// One refresh (the body of Swift `performRefresh`).
    pub async fn refresh_now(&self) -> RefreshOutcome {
        if !self.enabled.load(Ordering::SeqCst) || self.refreshing.swap(true, Ordering::SeqCst) {
            return RefreshOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let Some(cookie) = self.auth.cookie_header().filter(|c| !c.is_empty()) else {
            self.auth.set_needs_sign_in(true);
            let mut state = self.state();
            if state.snapshot.is_none() {
                state.last_error = Some(ProviderError::NotSignedIn(ProviderErrorContext::ChatGpt));
            }
            return RefreshOutcome::Skipped;
        };
        let generation = self.auth.state().session_generation;
        match (self.fetch)(cookie, generation).await {
            Ok(fetch) => self.on_success(fetch, generation),
            Err(error) => self.on_failure(error, generation),
        }
    }

    fn on_success(&self, fetch: Fetch, generation: SessionGeneration) -> RefreshOutcome {
        if !self.auth.is_current(generation) {
            return RefreshOutcome::Skipped;
        }
        self.auth.record_auth_success();
        self.auth.apply_refreshed_cookies(&fetch.set_cookie_headers);
        let snapshot = ChatGptSnapshot::new(fetch.response, fetch.fetched_at);
        tracing::info!(
            "ChatGPT refresh: {:.0}% used",
            snapshot.headline_used_percent()
        );
        let mut state = self.state();
        state.snapshot = Some(snapshot);
        state.last_error = None;
        state.last_refreshed_at = Some(self.clock.now());
        drop(state);
        self.auth.set_needs_sign_in(false);
        RefreshOutcome::Success
    }

    fn on_failure(&self, error: ProviderError, generation: SessionGeneration) -> RefreshOutcome {
        if !self.auth.is_current(generation) {
            return RefreshOutcome::Skipped;
        }
        if matches!(
            error.usage_error(),
            UsageError::Unauthorized | UsageError::NotSignedIn
        ) {
            self.token_cache.clear();
            self.auth.record_auth_failure(&error.to_string());
        }
        tracing::warn!("ChatGPT refresh failed: {error}");
        self.state().last_error = Some(error.clone());
        RefreshOutcome::Failed(error)
    }
}

impl AuthEventListener for ChatGptPoller {
    fn on_auth_event(&self, _event: AuthEvent) {
        self.clear_snapshot();
    }
}
