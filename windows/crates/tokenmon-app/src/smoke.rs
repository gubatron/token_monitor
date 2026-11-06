//! The `--smoke` run: start headless, run one refresh cycle, report, quit.
//!
//! CI executes the real `TokenMon.exe --smoke` on a Windows runner. Nothing is
//! signed in, so every poller finishes without touching the network.

use std::time::Duration;

use tokenmon_core::ProviderId;

use crate::model::AppModel;

/// Upper bound for the whole refresh cycle.
pub const SMOKE_TIMEOUT: Duration = Duration::from_secs(30);

/// What a smoke cycle observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeReport {
    /// Providers registered.
    pub providers: Vec<ProviderId>,
    /// Whether the refresh cycle finished within the timeout.
    pub completed: bool,
}

impl SmokeReport {
    /// Whether the run counts as a pass: a full registry and a finished cycle.
    pub fn passed(&self) -> bool {
        self.completed && self.providers.len() == ProviderId::USAGE_PROVIDERS.len()
    }
}

/// Runs one refresh of every provider (bounded by `timeout`) and quits the
/// model.
pub async fn run_refresh_cycle(model: &AppModel, timeout: Duration) -> SmokeReport {
    let providers = model
        .registry()
        .entries()
        .iter()
        .map(|e| e.provider)
        .collect();
    let completed = tokio::time::timeout(timeout, model.refresh_all())
        .await
        .is_ok();
    model.quit();
    SmokeReport {
        providers,
        completed,
    }
}
