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

/// The whole `--smoke` run: assembles the real app (real pollers, no network
/// because nothing is signed in) over a throwaway data directory and fake
/// platform collaborators, runs one refresh cycle and reports.
///
/// Returns `true` when the cycle passed.
pub fn run_headless(version: &str) -> bool {
    use std::sync::Arc;

    use tokenmon_core::SystemClock;
    use tokenmon_core::alerts::RecordingNotifier;
    use tokenmon_core::storage::AppPaths;
    use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, FakeEncryptor};
    use tokenmon_platform::webauth::FakeWebAuthHost;
    use tokenmon_provider_opencode::db::DataDirEnv;

    use crate::app::{App, AppInputs};

    let Ok(dir) = tempfile::tempdir() else {
        tracing::error!("smoke: no temp directory");
        return false;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        tracing::error!("smoke: no runtime");
        return false;
    };
    runtime.block_on(async {
        let inputs = AppInputs {
            paths: AppPaths::new(dir.path()),
            encryptor: Arc::new(FakeEncryptor::default()),
            fallback: APP_FALLBACK_POLICY,
            web_host: Arc::new(FakeWebAuthHost::default()),
            notifier: Arc::new(RecordingNotifier::default()),
            clock: Arc::new(SystemClock),
            version: version.to_owned(),
            home: Some(dir.path().to_path_buf()),
            opencode_env: DataDirEnv::default(),
        };
        let app = match App::build(inputs, tokio::runtime::Handle::current()) {
            Ok(app) => app,
            Err(e) => {
                tracing::error!(error = %e, "smoke: assembly failed");
                return false;
            }
        };
        let tasks = app.launch();
        let report = run_refresh_cycle(&app.model, SMOKE_TIMEOUT).await;
        for t in tasks {
            t.abort();
        }
        tracing::info!(?report, "smoke finished");
        report.passed()
    })
}
