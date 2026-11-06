//! `GrokbotUsagePoller` (spec 02 section 2.5): gating, the polling loop and
//! the publish step.
//!
//! Grokbot has no session of its own: it runs on the shared Cursor session,
//! so a rejected Grokbot call counts toward Cursor's three-strike counter
//! through core's [`ProviderRefresh`]. A non-challenge 403 is a custom error
//! and never a strike. The timing (interval, backoff, `Retry-After`, wake
//! gate) is core's [`PollingLoop`]. The refresh closures captured by the loop
//! hold only a [`Weak`] reference to the poller, so dropping the poller ends
//! the loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, ProviderRefresh};
use tokenmon_core::usage::{DailyBudgetDay, WeekOffset};
use tokenmon_core::{
    AuthEvent, AuthEventListener, BoxFuture, Clock, ProviderAuthSession, ProviderError,
    ProviderErrorContext, ProviderId, ProviderUsagePoller,
};

use crate::client::GrokbotUsageClient;
use crate::models::GrokbotSnapshot;
use crate::publish::GrokbotPublisher;

const CTX: ProviderErrorContext = ProviderErrorContext::Grokbot;

/// One fetch with the Cursor cookie header and the known account email (live:
/// [`GrokbotUsageClient`]; tests: a fake).
pub type FetchFn = Arc<
    dyn Fn(String, Option<String>) -> BoxFuture<'static, Result<GrokbotSnapshot, ProviderError>>
        + Send
        + Sync,
>;

/// Everything a [`GrokbotUsagePoller`] is wired to.
pub struct GrokbotPollerConfig {
    /// The shared Cursor session.
    pub session: Arc<dyn ProviderAuthSession>,
    /// History stores and last snapshot.
    pub publisher: GrokbotPublisher,
    /// "Now".
    pub clock: Arc<dyn Clock>,
    /// The fetch seam.
    pub fetch: FetchFn,
    /// `AppSettings.isProviderEnabled(.grokbot)`.
    pub is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `AppSettings.pollInterval(for: .grokbot, menuIsOpen:)`; `None` parks.
    pub poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
}

impl GrokbotPollerConfig {
    /// Config whose fetch is the live `client`.
    pub fn live(
        session: Arc<dyn ProviderAuthSession>,
        publisher: GrokbotPublisher,
        clock: Arc<dyn Clock>,
        client: GrokbotUsageClient,
        is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
        poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    ) -> Self {
        let client = Arc::new(client);
        let fetch_clock = Arc::clone(&clock);
        Self {
            session,
            publisher,
            clock,
            fetch: Arc::new(move |cookie, email| {
                let client = Arc::clone(&client);
                let now = fetch_clock.now();
                Box::pin(async move {
                    client
                        .fetch_usage_status(&cookie, email.as_deref(), now)
                        .await
                })
            }),
            is_enabled,
            poll_interval,
        }
    }
}

#[derive(Default)]
struct Status {
    last_error: Option<ProviderError>,
    last_refreshed_at: Option<DateTime<Utc>>,
}

/// Clears `refreshing` when a refresh ends, however it ends.
struct RefreshGuard<'a>(&'a AtomicBool);

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

struct SessionEvents(Weak<GrokbotUsagePoller>);

impl AuthEventListener for SessionEvents {
    fn on_auth_event(&self, event: AuthEvent) {
        let Some(poller) = self.0.upgrade() else {
            return;
        };
        poller.clear_snapshot();
        if event == AuthEvent::AccountReset {
            lock(&poller.publisher).account_reset();
        }
    }
}

/// Polls the Grokbot allowance over the shared Cursor session.
pub struct GrokbotUsagePoller {
    session: Arc<dyn ProviderAuthSession>,
    publisher: Mutex<GrokbotPublisher>,
    clock: Arc<dyn Clock>,
    fetch: FetchFn,
    is_enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    poll_interval: Arc<dyn Fn(bool) -> Option<Duration> + Send + Sync>,
    status: Mutex<Status>,
    refreshing: AtomicBool,
    menu_open: AtomicBool,
    polling: Arc<PollingLoop>,
}

impl GrokbotUsagePoller {
    /// Wires the poller to its session and builds (but does not start) its
    /// polling loop.
    ///
    /// Signing out drops the snapshot; an account change also clears the
    /// history stores.
    ///
    /// # Panics
    /// Outside a tokio runtime (the loop spawns its waits on it).
    pub fn new(config: GrokbotPollerConfig) -> Arc<Self> {
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
                session: config.session,
                publisher: Mutex::new(config.publisher),
                clock: config.clock,
                fetch: config.fetch,
                is_enabled: config.is_enabled,
                poll_interval: config.poll_interval,
                status: Mutex::new(Status::default()),
                refreshing: AtomicBool::new(false),
                menu_open: AtomicBool::new(false),
                polling,
            }
        });
        poller
            .session
            .add_listener(Arc::new(SessionEvents(Arc::downgrade(&poller))));
        poller
    }

    /// The polling loop (wake, pause and resume for the app shell).
    pub fn polling_loop(&self) -> &Arc<PollingLoop> {
        &self.polling
    }

    /// The last published snapshot.
    pub fn snapshot(&self) -> Option<GrokbotSnapshot> {
        lock(&self.publisher).snapshot().cloned()
    }

    /// The Monday-week bars `week_offset` steps from the current week.
    pub fn daily_budget_days(&self, week_offset: WeekOffset) -> Vec<DailyBudgetDay> {
        lock(&self.publisher).daily_budget_days(week_offset)
    }

    /// Runs `f` on the publisher (history stores) under its lock.
    pub fn with_publisher<R>(&self, f: impl FnOnce(&GrokbotPublisher) -> R) -> R {
        f(&lock(&self.publisher))
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

    /// One refresh: gating, the guarded fetch and the publish step.
    ///
    /// Skips when the provider is disabled or a refresh is running. Without a
    /// cookie it flags `needs_sign_in` and skips.
    pub async fn perform_refresh(&self) -> PollOutcome {
        if !(self.is_enabled)() || self.refreshing.swap(true, Ordering::SeqCst) {
            return PollOutcome::Skipped;
        }
        let _guard = RefreshGuard(&self.refreshing);
        let Some(cookie) = self.session.cookie_header().filter(|c| !c.is_empty()) else {
            self.session.set_needs_sign_in(true);
            let no_snapshot = lock(&self.publisher).snapshot().is_none();
            if no_snapshot {
                lock(&self.status).last_error = Some(ProviderError::NotSignedIn(CTX));
            }
            return PollOutcome::Skipped;
        };
        let email = self.session.state().account_email;
        let failure: Mutex<Option<ProviderError>> = Mutex::new(None);
        ProviderRefresh::run(
            &*self.session,
            "Grokbot",
            || async {
                let result = (self.fetch)(cookie, email).await;
                if let Err(error) = &result {
                    *lock(&failure) = Some(error.clone());
                }
                result
            },
            |snapshot| self.publish(snapshot),
            || {},
            |_message| lock(&self.status).last_error = lock(&failure).take(),
        )
        .await
    }

    fn publish(&self, snapshot: GrokbotSnapshot) -> PollOutcome {
        tracing::info!(
            "Grokbot refresh: {:.1}% used ({:.0}% left)",
            snapshot.used_percent,
            snapshot.remaining_percent().round()
        );
        lock(&self.publisher).publish(snapshot);
        {
            let mut status = lock(&self.status);
            status.last_error = None;
            status.last_refreshed_at = Some(self.clock.now());
        }
        self.session.set_needs_sign_in(false);
        PollOutcome::Success
    }
}

impl ProviderUsagePoller for GrokbotUsagePoller {
    fn provider(&self) -> ProviderId {
        ProviderId::Grokbot
    }

    fn set_menu_open(&self, open: bool) {
        if self.menu_open.swap(open, Ordering::SeqCst) != open {
            self.polling.wake();
        }
    }

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        lock(&self.status).last_refreshed_at
    }

    fn last_error(&self) -> Option<ProviderError> {
        lock(&self.status).last_error.clone()
    }

    fn headline_used_percent(&self) -> Option<f64> {
        lock(&self.publisher).snapshot().map(|s| s.used_percent)
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        lock(&self.publisher).snapshot().and_then(|s| s.resets_at)
    }

    fn alert_account(&self) -> Option<String> {
        self.session.state().account_email
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

    fn clear_snapshot(&self) {
        lock(&self.publisher).clear_snapshot();
        *lock(&self.status) = Status::default();
    }
}
