//! Grok on the shared polling engine (spec 02 section 1.6, spec 01 section 1.2).
//!
//! [`GrokProviderPoller`] pairs a [`GrokUsagePoller`] (state and guarded
//! refresh) with a core [`PollingLoop`] and implements
//! [`ProviderUsagePoller`]. The loop's interval comes from
//! [`Settings::poll_interval`], so it parks (no timer) while nothing needs Grok
//! data. The refresh closure holds only a [`Weak`] reference to the poller
//! state, so a dropped provider never stays alive through its own loop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use chrono::{DateTime, Utc};
use tokenmon_core::polling::{PollOutcome, PollingLoop, PollingLoopBuilder};
use tokenmon_core::settings::Settings;
use tokenmon_core::{BoxFuture, ProviderError, ProviderId, ProviderUsagePoller};

use crate::poller::{GrokUsagePoller, RefreshOutcome};

/// Supplies the current settings snapshot on every interval decision.
pub type SettingsSource = Arc<dyn Fn() -> Settings + Send + Sync>;

/// Maps one refresh result to the loop's backoff outcome.
fn outcome_for(poller: &GrokUsagePoller, result: RefreshOutcome) -> PollOutcome {
    match result {
        RefreshOutcome::Published => PollOutcome::Success,
        RefreshOutcome::Skipped => PollOutcome::Skipped,
        RefreshOutcome::Failed => poller
            .last_error()
            .map_or(PollOutcome::Failure { retry_after: None }, |e| {
                PollOutcome::from(&e)
            }),
    }
}

/// Grok's [`ProviderUsagePoller`]: state plus a gated polling loop.
pub struct GrokProviderPoller {
    inner: Arc<GrokUsagePoller>,
    menu_open: Arc<AtomicBool>,
    polling_loop: Arc<PollingLoop>,
}

impl GrokProviderPoller {
    /// Builds the poller. `configure` customizes the loop (clock, sleeper,
    /// jitter, runtime handle); pass `|b| b` for the production defaults.
    pub fn new(
        inner: Arc<GrokUsagePoller>,
        settings: SettingsSource,
        configure: impl FnOnce(PollingLoopBuilder) -> PollingLoopBuilder,
    ) -> Self {
        let menu_open = Arc::new(AtomicBool::new(false));
        let enabled_source = Arc::clone(&settings);
        let interval = {
            let menu_open = Arc::clone(&menu_open);
            move || settings().poll_interval(ProviderId::Grok, menu_open.load(Ordering::SeqCst))
        };
        let weak: Weak<GrokUsagePoller> = Arc::downgrade(&inner);
        let refresh = move || {
            let weak = weak.clone();
            let enabled_source = Arc::clone(&enabled_source);
            async move {
                match weak.upgrade() {
                    // Gate every refresh (timer, refresh_now, sign-in
                    // completion) on the setting, like Swift's
                    // `isProviderEnabled(.grok)` check.
                    Some(_) if !enabled_source().is_provider_enabled(ProviderId::Grok) => {
                        PollOutcome::Skipped
                    }
                    Some(poller) => {
                        let result = poller.perform_refresh().await;
                        outcome_for(&poller, result)
                    }
                    None => PollOutcome::Skipped,
                }
            }
        };
        let polling_loop = configure(PollingLoop::builder(interval, refresh)).build();
        Self {
            inner,
            menu_open,
            polling_loop,
        }
    }

    /// The wrapped state holder (snapshot, week bars, history).
    pub fn usage(&self) -> &Arc<GrokUsagePoller> {
        &self.inner
    }

    /// The timed loop, for pause/resume on system sleep and for tests.
    pub fn polling_loop(&self) -> &Arc<PollingLoop> {
        &self.polling_loop
    }

    /// Re-evaluates the interval after a settings change.
    pub fn settings_changed(&self) {
        self.polling_loop.wake();
    }
}

impl ProviderUsagePoller for GrokProviderPoller {
    fn provider(&self) -> ProviderId {
        ProviderId::Grok
    }

    fn set_menu_open(&self, open: bool) {
        if self.menu_open.swap(open, Ordering::SeqCst) != open {
            self.polling_loop.wake();
        }
    }

    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        self.inner.last_refreshed_at()
    }

    fn last_error(&self) -> Option<ProviderError> {
        self.inner.last_error()
    }

    fn headline_used_percent(&self) -> Option<f64> {
        self.inner.headline_used_percent()
    }

    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        self.inner.headline_resets_at()
    }

    fn alert_account(&self) -> Option<String> {
        self.inner.alert_account()
    }

    fn start(&self) {
        self.polling_loop.start();
    }

    fn stop(&self) {
        self.polling_loop.stop();
    }

    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.polling_loop.refresh_now().await })
    }

    fn clear_snapshot(&self) {
        self.inner.clear_snapshot();
    }
}
