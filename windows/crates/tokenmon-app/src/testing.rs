//! Portable fakes for driving the app model without real providers.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokenmon_core::{BoxFuture, ProviderError, ProviderId, ProviderUsagePoller};

use crate::poller::ManagedPoller;

/// One call a [`FakePoller`] observed, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PollerCall {
    /// `start()`.
    Start,
    /// `stop()`.
    Stop,
    /// `wake()`.
    Wake,
    /// `pause()`.
    Pause,
    /// `resume()`.
    Resume,
    /// `refresh_now()`.
    RefreshNow,
    /// `refresh_now_if_older_than(age)`.
    RefreshIfOlder(Duration),
    /// `clear_snapshot()`.
    ClearSnapshot,
}

/// A recording [`ManagedPoller`] with scriptable headline data.
pub struct FakePoller {
    provider: ProviderId,
    calls: Mutex<Vec<PollerCall>>,
    menu_open: AtomicBool,
    headline: Mutex<Option<(f64, Option<DateTime<Utc>>)>>,
    refresh_count: AtomicU32,
}

impl FakePoller {
    /// A fake for `provider` with no data.
    pub fn new(provider: ProviderId) -> Arc<Self> {
        Arc::new(Self {
            provider,
            calls: Mutex::new(Vec::new()),
            menu_open: AtomicBool::new(false),
            headline: Mutex::new(None),
            refresh_count: AtomicU32::new(0),
        })
    }

    /// Sets the headline percentage (and reset time) the fake reports.
    pub fn set_headline(&self, percent: f64, resets_at: Option<DateTime<Utc>>) {
        *self.headline.lock().unwrap_or_else(PoisonError::into_inner) = Some((percent, resets_at));
    }

    /// Every call so far, oldest first.
    pub fn calls(&self) -> Vec<PollerCall> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many times `call` happened.
    pub fn count(&self, call: PollerCall) -> usize {
        self.calls().into_iter().filter(|c| *c == call).count()
    }

    /// Number of `refresh_now` calls (also counts conditional refreshes that ran).
    pub fn refreshes(&self) -> u32 {
        self.refresh_count.load(Ordering::SeqCst)
    }

    /// The last value passed to `set_menu_open`.
    pub fn menu_is_open(&self) -> bool {
        self.menu_open.load(Ordering::SeqCst)
    }

    fn record(&self, call: PollerCall) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

impl ProviderUsagePoller for FakePoller {
    fn provider(&self) -> ProviderId {
        self.provider
    }
    fn set_menu_open(&self, open: bool) {
        self.menu_open.store(open, Ordering::SeqCst);
    }
    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        None
    }
    fn last_error(&self) -> Option<ProviderError> {
        None
    }
    fn headline_used_percent(&self) -> Option<f64> {
        self.headline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|(p, _)| p)
    }
    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        self.headline
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .and_then(|(_, r)| r)
    }
    fn alert_account(&self) -> Option<String> {
        None
    }
    fn start(&self) {
        self.record(PollerCall::Start);
    }
    fn stop(&self) {
        self.record(PollerCall::Stop);
    }
    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.record(PollerCall::RefreshNow);
            self.refresh_count.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn clear_snapshot(&self) {
        self.record(PollerCall::ClearSnapshot);
    }
}

impl ManagedPoller for FakePoller {
    fn wake(&self) {
        self.record(PollerCall::Wake);
    }
    fn pause(&self) {
        self.record(PollerCall::Pause);
    }
    fn resume(&self) {
        self.record(PollerCall::Resume);
    }
    fn refresh_now_if_older_than(&self, age: Duration) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.record(PollerCall::RefreshIfOlder(age));
            self.refresh_count.fetch_add(1, Ordering::SeqCst);
        })
    }
}
