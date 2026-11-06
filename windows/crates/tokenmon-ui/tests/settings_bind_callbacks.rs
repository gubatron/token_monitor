//! The Settings window's callbacks reach the view model and the window is
//! refreshed from the backend's persisted truth afterwards.
#![allow(missing_docs, clippy::unwrap_used)]

mod common;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use tokenmon_core::ProviderId;
use tokenmon_ui::settings::bind::create_settings_window;
use tokenmon_ui::settings::fake::{FixedSaveDialog, HostCall, InMemorySettings, RecordingHost};
use tokenmon_ui::settings::{ExportFormat, SettingsBackend, SettingsSnapshot, SettingsViewModel};

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Drives every callback of the settings window (one test: Slint platform
/// state is per process) and checks the host calls and persisted settings.
#[test]
fn test_settings_callbacks_route_to_view_model() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let settings = Arc::new(InMemorySettings::new(SettingsSnapshot::default()));
    let host = Arc::new(RecordingHost::default());
    let dialog = Arc::new(FixedSaveDialog {
        answer: Some(PathBuf::from("out.csv")),
        ..FixedSaveDialog::default()
    });
    let vm = Arc::new(SettingsViewModel::new(
        settings.clone(),
        host.clone(),
        dialog.clone(),
    ));
    let w = create_settings_window(vm).unwrap();

    w.invoke_launch_at_login_changed(true);
    w.invoke_desktop_menu_changed(true);
    w.invoke_update_action();
    w.invoke_sign_in("grok".into());
    w.invoke_sign_in("not-a-provider".into());
    w.invoke_sign_out("cursor".into());
    w.invoke_openrouter_sign_out();
    w.invoke_clear_history();
    w.invoke_export_history("csv".into());
    // An unknown export format never opens the dialog or calls the host.
    w.invoke_export_history("xml".into());
    w.invoke_quit();
    assert_eq!(
        host.calls(),
        vec![
            HostCall::LaunchAtLogin(true),
            HostCall::DesktopMenu(true),
            HostCall::UpdateAction,
            HostCall::SignIn(ProviderId::Grok),
            HostCall::SignOut(ProviderId::Cursor),
            HostCall::SignOutOpenRouter,
            HostCall::ClearHistory,
            HostCall::Export(ExportFormat::Csv, PathBuf::from("out.csv")),
            HostCall::Quit,
        ]
    );
    assert_eq!(dialog.requested.lock().unwrap().len(), 1);

    // Settings-backed callbacks persist and the stepper direction is honoured.
    w.invoke_checks_for_updates_changed(false);
    w.invoke_threshold_enabled_changed(true);
    w.invoke_threshold_changed(90.0);
    w.invoke_active_step(-1);
    w.invoke_idle_step(1);
    w.invoke_show_selected_only_changed(true);
    let snap = settings.snapshot();
    assert!(!snap.checks_for_updates);
    assert!(snap.threshold_enabled);
    assert_eq!(snap.threshold_percent, 90.0);
    assert_eq!(snap.active_poll_seconds, 45);
    assert_eq!(snap.idle_poll_seconds, 360);
    assert!(snap.show_selected_provider_only);

    // The window re-reads the clamped value after a change.
    w.invoke_threshold_changed(10.0);
    assert_eq!(settings.snapshot().threshold_percent, 50.0);

    check_openrouter_save_clears_draft_only_when_accepted();
}

/// A saved OpenRouter key clears the draft; a rejected one keeps it.
fn check_openrouter_save_clears_draft_only_when_accepted() {
    let f = common::fixture(None);
    let vm = Arc::new(SettingsViewModel::new(
        f.settings.clone(),
        f.host.clone(),
        f.dialog.clone(),
    ));
    let w = create_settings_window(vm).unwrap();
    w.set_openrouter_draft("sk-or-v1-abc".into());
    w.invoke_openrouter_save("sk-or-v1-abc".into());
    assert_eq!(w.get_openrouter_draft().as_str(), "");
    assert_eq!(
        f.host.calls(),
        vec![HostCall::SaveOpenRouterKey("sk-or-v1-abc".into())]
    );

    *f.host.fail_with.lock().unwrap() = Some("bad key".into());
    w.set_openrouter_draft("sk-or-v1-bad".into());
    w.invoke_openrouter_save("sk-or-v1-bad".into());
    assert_eq!(w.get_openrouter_draft().as_str(), "sk-or-v1-bad");
}
