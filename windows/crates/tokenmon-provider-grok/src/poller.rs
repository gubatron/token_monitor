//! Grok poller state and refresh gating (spec 02 section 1.6).
//!
//! This is the provider's half of polling: enabled and signed-in gating, the
//! session-generation guard, publish into history, and the displayed state.
//! The timed loop (interval, wake, backoff) is the shared polling engine's;
//! it calls [`GrokUsagePoller::perform_refresh`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, BoxFuture, Clock, ProviderError,
    ProviderErrorContext,
};

use crate::calendar::LocalCalendar;
use crate::daily;
use crate::history::GrokHistoryStore;
use crate::models::{DailyUsageWeek, WeeklyUsageSnapshot};

/// One network round trip for the poller.
pub trait GrokFetch: Send + Sync {
    /// Fetches usage with the given cookie header and account email.
    fn fetch(
        &self,
        cookie_header: String,
        account_email: Option<String>,
    ) -> BoxFuture<'_, Result<WeeklyUsageSnapshot, ProviderError>>;
}

/// Receives the used% of each good poll (the hourly activity store).
pub trait HourlySink: Send + Sync {
    /// Records `used_percent` observed at `at`.
    fn record(&self, used_percent: f64, at: DateTime<Utc>);
    /// Drops all hourly data (a different account signed in).
    fn clear(&self);
}

/// What a refresh attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// A snapshot was published.
    Published,
    /// Skipped: disabled, already refreshing, not signed in, or stale.
    Skipped,
    /// The fetch failed and the error was recorded.
    Failed,
}

#[derive(Default)]
struct State {
    snapshot: Option<WeeklyUsageSnapshot>,
    last_error: Option<ProviderError>,
    last_refreshed_at: Option<DateTime<Utc>>,
}

/// Grok poller: displayed state plus the guarded refresh.
pub struct GrokUsagePoller {
    auth: Arc<dyn AuthSession>,
    history: Arc<Mutex<GrokHistoryStore>>,
    clock: Arc<dyn Clock>,
    fetcher: Arc<dyn GrokFetch>,
    hourly: Option<Arc<dyn HourlySink>>,
    enabled: AtomicBool,
    refreshing: AtomicBool,
    state: Arc<Mutex<State>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Clears the refreshing flag when the refresh ends or is dropped.
struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl GrokUsagePoller {
    /// A poller. `enabled` starts true.
    pub fn new(
        auth: Arc<dyn AuthSession>,
        history: GrokHistoryStore,
        clock: Arc<dyn Clock>,
        fetcher: Arc<dyn GrokFetch>,
        hourly: Option<Arc<dyn HourlySink>>,
    ) -> Self {
        let poller = Self {
            auth,
            history: Arc::new(Mutex::new(history)),
            clock,
            fetcher,
            hourly,
            enabled: AtomicBool::new(true),
            refreshing: AtomicBool::new(false),
            state: Arc::new(Mutex::new(State::default())),
        };
        let email = poller.auth.state().account_email;
        lock(&poller.history).set_active_account(email.as_deref());
        // The listener owns clones of the shared state it clears, never the
        // poller or the session, so the session's listener list cannot keep a
        // reference cycle alive.
        poller.auth.add_listener(Arc::new(SessionEvents {
            state: Arc::clone(&poller.state),
            hourly: poller.hourly.clone(),
        }));
        poller
    }

    /// Enables or disables the provider; a disabled poller never fetches.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    /// Runs one guarded refresh.
    ///
    /// Skips when disabled or already refreshing. When not signed in it records
    /// "Sign in to grok.com to load usage." and skips. A result is discarded
    /// when the session generation changed during the fetch; on success the
    /// auth failure streak resets and the snapshot is published. A rejection
    /// (`Unauthorized`/`NotSignedIn`) counts toward invalidating the session.
    /// Dropping the returned future cancels the fetch without recording an error.
    pub async fn perform_refresh(&self) -> RefreshOutcome {
        if !self.enabled.load(Ordering::SeqCst) || self.refreshing.swap(true, Ordering::SeqCst) {
            return RefreshOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let session = self.auth.state();
        let cookie = self.auth.cookie_header();
        let Some(cookie) = cookie.filter(|_| session.is_signed_in && !session.needs_sign_in) else {
            lock(&self.state).last_error =
                Some(ProviderError::NotSignedIn(ProviderErrorContext::Grok));
            return RefreshOutcome::Skipped;
        };
        let generation = session.session_generation;
        let result = self.fetcher.fetch(cookie, session.account_email).await;
        if !self.auth.is_current(generation) {
            return RefreshOutcome::Skipped;
        }
        match result {
            Ok(snapshot) => {
                self.auth.record_auth_success();
                self.publish(snapshot);
                RefreshOutcome::Published
            }
            Err(error) => {
                if matches!(
                    error,
                    ProviderError::Unauthorized(_) | ProviderError::NotSignedIn(_)
                ) {
                    self.auth.record_auth_failure(&error.to_string());
                }
                lock(&self.state).last_error = Some(error);
                RefreshOutcome::Failed
            }
        }
    }

    fn publish(&self, mut snapshot: WeeklyUsageSnapshot) {
        if snapshot.account_email.is_none() {
            snapshot.account_email = self.auth.state().account_email;
        }
        let now = self.clock.now();
        {
            let mut state = lock(&self.state);
            state.snapshot = Some(snapshot.clone());
            state.last_error = None;
            state.last_refreshed_at = Some(now);
        }
        if let Some(hourly) = &self.hourly {
            hourly.record(snapshot.used_percent, snapshot.fetched_at);
        }
        lock(&self.history).append(snapshot);
    }

    /// The last good snapshot (kept visible after a failed poll).
    pub fn snapshot(&self) -> Option<WeeklyUsageSnapshot> {
        lock(&self.state).snapshot.clone()
    }

    /// The last refresh error, cleared by the next success.
    pub fn last_error(&self) -> Option<ProviderError> {
        lock(&self.state).last_error.clone()
    }

    /// When the last successful refresh was published.
    pub fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        lock(&self.state).last_refreshed_at
    }

    /// Headline used% (menu bar and alerts).
    pub fn headline_used_percent(&self) -> Option<f64> {
        self.snapshot().map(|s| s.used_percent)
    }

    /// Headline reset instant.
    pub fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        self.snapshot().and_then(|s| s.resets_at)
    }

    /// Account label for alerts.
    pub fn alert_account(&self) -> Option<String> {
        self.auth.state().account_email
    }

    /// Drops the displayed snapshot, error and refresh time; history and
    /// hourly data are kept.
    pub fn clear_snapshot(&self) {
        *lock(&self.state) = State::default();
    }

    /// The daily-bar week for `week_offset` (`0` is the current period):
    /// local history (oldest first) plus the current snapshot. `None` without a
    /// snapshot or provider reset.
    pub fn week(&self, week_offset: i64) -> Option<DailyUsageWeek> {
        let snapshot = self.snapshot()?;
        let history: Vec<WeeklyUsageSnapshot> =
            lock(&self.history).recent().iter().rev().cloned().collect();
        let cal = LocalCalendar::new(self.clock.as_ref());
        daily::week(
            &history,
            Some(&snapshot),
            week_offset,
            snapshot.resets_at,
            &cal,
            self.clock.now(),
        )
    }

    /// Writes the history to disk when it changed (call on quit and after
    /// sign-out; a no-op when clean or when the file could not be read).
    pub fn flush_history(&self) {
        lock(&self.history).flush();
    }

    /// Runs `f` with the history store (export, clear, flush).
    pub fn with_history<R>(&self, f: impl FnOnce(&mut GrokHistoryStore) -> R) -> R {
        f(&mut lock(&self.history))
    }
}

/// Clears displayed state on session events (Swift `GrokUsagePoller.init`
/// sinks): sign-out drops the snapshot; a different account also drops the
/// hourly activity so the old account's hours never merge into the new one's.
struct SessionEvents {
    state: Arc<Mutex<State>>,
    hourly: Option<Arc<dyn HourlySink>>,
}

impl AuthEventListener for SessionEvents {
    fn on_auth_event(&self, event: AuthEvent) {
        *lock(&self.state) = State::default();
        if event == AuthEvent::AccountReset
            && let Some(hourly) = &self.hourly
        {
            hourly.clear();
        }
    }
}
