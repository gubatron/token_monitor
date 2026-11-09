//! Assembles the whole app over fakes and runs the launch and smoke paths.

use std::sync::Arc;
use std::time::Duration;

use tokenmon_app::app::{App, AppInputs};
use tokenmon_app::smoke::run_refresh_cycle;
use tokenmon_core::alerts::RecordingNotifier;
use tokenmon_core::storage::AppPaths;
use tokenmon_core::{ProviderId, SystemClock};
use tokenmon_platform::secrets::FakeEncryptor;
use tokenmon_platform::webauth::FakeWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;

fn inputs(dir: &std::path::Path) -> AppInputs {
    AppInputs {
        paths: AppPaths::new(dir),
        encryptor: Arc::new(FakeEncryptor::default()),
        web_host: Arc::new(FakeWebAuthHost::default()),
        notifier: Arc::new(RecordingNotifier::default()),
        clock: Arc::new(SystemClock),
        version: "0.0.0-test".to_owned(),
        home: Some(dir.to_path_buf()),
        opencode_env: DataDirEnv::default(),
    }
}

/// The registry has all seven providers in order and Grokbot shares Cursor's
/// session object.
#[tokio::test]
async fn test_build_registers_every_provider_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = App::build(inputs(dir.path()), tokio::runtime::Handle::current()).expect("builds");
    let order: Vec<_> = app
        .model
        .registry()
        .entries()
        .iter()
        .map(|e| e.provider)
        .collect();
    assert_eq!(order, ProviderId::USAGE_PROVIDERS);
    assert_eq!(
        app.model
            .registry()
            .sharing_session_with(ProviderId::Cursor)
            .len(),
        2
    );
}

/// Launching, then one refresh cycle signed out, finishes without a network
/// and quits cleanly: this is exactly what `TokenMon.exe --smoke` does.
#[tokio::test]
async fn test_launch_and_signed_out_refresh_cycle_passes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = App::build(inputs(dir.path()), tokio::runtime::Handle::current()).expect("builds");
    let tasks = app.launch();
    let report = run_refresh_cycle(&app.model, Duration::from_secs(20)).await;
    for t in tasks {
        t.abort();
    }
    assert!(report.passed(), "{report:?}");
}

/// The wake gate callbacks pause and resume the model's loops without panic.
#[tokio::test]
async fn test_wake_callbacks_drive_the_model() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = App::build(inputs(dir.path()), tokio::runtime::Handle::current()).expect("builds");
    let cb = app.wake_callbacks();
    (cb.on_sleep)();
    (cb.on_ready)();
    app.model.quit();
}
