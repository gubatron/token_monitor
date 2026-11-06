//! Shared fixtures for the Settings tests.
#![allow(dead_code, missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;

use tokenmon_ui::settings::fake::{FixedSaveDialog, InMemorySettings, RecordingHost};
use tokenmon_ui::settings::{SettingsSnapshot, SettingsViewModel};

/// View model plus the fakes behind it.
pub struct Fixture {
    pub settings: Arc<InMemorySettings>,
    pub host: Arc<RecordingHost>,
    pub dialog: Arc<FixedSaveDialog>,
    pub vm: SettingsViewModel,
}

/// Builds a fixture; `save_path` is what the save dialog answers (`None` cancels).
pub fn fixture(save_path: Option<&str>) -> Fixture {
    let settings = Arc::new(InMemorySettings::new(SettingsSnapshot::default()));
    let host = Arc::new(RecordingHost::default());
    let dialog = Arc::new(FixedSaveDialog {
        answer: save_path.map(PathBuf::from),
        ..FixedSaveDialog::default()
    });
    let vm = SettingsViewModel::new(settings.clone(), host.clone(), dialog.clone());
    Fixture {
        settings,
        host,
        dialog,
        vm,
    }
}
