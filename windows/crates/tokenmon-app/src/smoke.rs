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
    use tokenmon_platform::secrets::FakeEncryptor;
    use tokenmon_platform::webauth::FakeWebAuthHost;
    use tokenmon_provider_opencode::db::DataDirEnv;

    use crate::app::{App, AppInputs};

    if !own_version_resource_matches(version) {
        tracing::error!("smoke: the exe's version resource does not match {version}");
        return false;
    }

    if !own_icon_resource_present() {
        tracing::error!("smoke: the exe has no application icon (RT_GROUP_ICON) resource");
        return false;
    }

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
            web_host: Arc::new(FakeWebAuthHost::default()),
            notifier: Arc::new(RecordingNotifier::default()),
            clock: Arc::new(SystemClock),
            version: version.to_owned(),
            home: Some(dir.path().to_path_buf()),
            claude_config_dir: None,
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

/// The updater trusts the version resource of a downloaded exe (#77), so every
/// shipped exe must carry one equal to the build's own version (`build.rs`).
/// Only checked for the MSVC build the releases ship.
#[cfg(all(windows, target_env = "msvc"))]
fn own_version_resource_matches(version: &str) -> bool {
    use tokenmon_core::update::AppVersion;

    let declared = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::File::open(p).ok())
        .and_then(tokenmon_platform::install::read_pe_version);
    declared.is_some() && declared == AppVersion::parse(version)
}

#[cfg(not(all(windows, target_env = "msvc")))]
fn own_version_resource_matches(_version: &str) -> bool {
    true
}

/// Without an icon group the taskbar, Alt+Tab and Explorer show the generic
/// application icon (#81); `build.rs` embeds `tokenmon.ico` as group id 1.
#[cfg(all(windows, target_env = "msvc"))]
fn own_icon_resource_present() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::File::open(p).ok())
        .is_some_and(tokenmon_platform::install::pe_has_icon_group)
}

#[cfg(not(all(windows, target_env = "msvc")))]
fn own_icon_resource_present() -> bool {
    true
}
