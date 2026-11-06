//! Threshold alerts: feeds every poller's headline into the core
//! [`ThresholdNotifier`] (spec 01 section 5, "Wiring").
//!
//! The Mac observes `headlineUsedPercent`; there is no observation framework
//! here, so [`AlertWatcher::spawn`] re-evaluates on a short tick. That is safe
//! because [`ThresholdNotifier::evaluate`] is idempotent: it records the last
//! notified threshold, so repeated evaluation of the same value stays quiet.

use std::sync::Arc;
use std::time::Duration;

use tokenmon_core::alerts::ThresholdNotifier;
use tokenmon_core::settings::AppSettings;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::registry::ProviderRegistry;

/// How often headlines are re-evaluated.
pub const EVALUATION_TICK: Duration = Duration::from_secs(2);

/// Evaluates usage alerts for every registered provider.
#[derive(Clone)]
pub struct AlertWatcher {
    registry: ProviderRegistry,
    settings: Arc<AppSettings>,
    notifier: ThresholdNotifier,
}

impl AlertWatcher {
    /// A watcher over `registry`.
    pub fn new(
        registry: ProviderRegistry,
        settings: Arc<AppSettings>,
        notifier: ThresholdNotifier,
    ) -> Self {
        Self {
            registry,
            settings,
            notifier,
        }
    }

    /// Evaluates every provider that has a headline percentage.
    pub fn evaluate_all(&self) {
        let settings = self.settings.snapshot();
        for e in self.registry.entries() {
            let Some(used) = e.poller.headline_used_percent() else {
                continue;
            };
            self.notifier.evaluate(
                e.provider,
                used,
                &settings,
                e.poller.alert_account().as_deref(),
                e.poller.headline_resets_at(),
            );
        }
    }

    /// Evaluates every [`EVALUATION_TICK`] on `handle` until aborted.
    pub fn spawn(self, handle: &Handle) -> JoinHandle<()> {
        handle.spawn(async move {
            let mut tick = tokio::time::interval(EVALUATION_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                self.evaluate_all();
            }
        })
    }
}
