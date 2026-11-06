//! The production update service wires up without touching the network.

use std::sync::Arc;

use tokenmon_app::updates::UpdateService;
use tokenmon_core::settings::{AppSettings, SettingsFile};
use tokenmon_core::update::UrlOpener;

struct NoOpener;

impl UrlOpener for NoOpener {
    fn open(&self, _url: &str) {}
}

/// Building the service starts nothing: the checker has no release and is not
/// checking until the loop runs.
#[tokio::test]
async fn test_production_update_service_starts_idle() {
    let settings = Arc::new(AppSettings::new(Arc::new(SettingsFile::in_memory())));
    let service = UpdateService::production(
        &settings,
        "1.0.0",
        std::path::PathBuf::from("TokenMon.exe"),
        Arc::new(NoOpener),
    )
    .expect("builds");
    let state = service.checker.state();
    assert!(state.available_release.is_none());
    assert!(!state.is_checking);
    assert!(service.checker.settings_changed());
    settings.set_checks_for_updates(false);
    assert!(!service.checker.settings_changed());
}
