//! In-memory adapters for tests and the offscreen snapshots. They mirror the
//! spec 01 §2 clamps so the view model is exercised end to end without
//! `AppSettings`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokenmon_core::ProviderId;

use super::model::{
    ACTIVE_POLL_RANGE, ExportFormat, HostStatus, IDLE_POLL_RANGE, SaveDialog, SettingsBackend,
    SettingsChange, SettingsHost, SettingsSnapshot,
};

/// [`SettingsBackend`] holding a [`SettingsSnapshot`] in memory.
#[derive(Debug, Default)]
pub struct InMemorySettings {
    state: Mutex<SettingsSnapshot>,
}

impl InMemorySettings {
    /// Starts from `snapshot`.
    pub fn new(snapshot: SettingsSnapshot) -> Self {
        Self {
            state: Mutex::new(snapshot),
        }
    }
}

fn set_membership(list: &mut Vec<ProviderId>, p: ProviderId, on: bool) {
    list.retain(|x| *x != p);
    if on {
        list.push(p);
        list.sort();
    }
}

impl SettingsBackend for InMemorySettings {
    fn snapshot(&self) -> SettingsSnapshot {
        self.state.lock().expect("settings lock").clone()
    }

    fn apply(&self, change: SettingsChange) {
        let mut s = self.state.lock().expect("settings lock");
        match change {
            SettingsChange::ShowCategoriesInMenuBar(v) => s.show_categories_in_menu_bar = v,
            SettingsChange::MenuBarGraph(p, on) => {
                set_membership(&mut s.menu_bar_graph_providers, p, on);
            }
            SettingsChange::ShowSelectedProviderOnly(v) => s.show_selected_provider_only = v,
            SettingsChange::ChecksForUpdates(v) => s.checks_for_updates = v,
            SettingsChange::ActivePollSeconds(v) => {
                s.active_poll_seconds = v.clamp(ACTIVE_POLL_RANGE.0, ACTIVE_POLL_RANGE.1);
            }
            SettingsChange::IdlePollSeconds(v) => {
                s.idle_poll_seconds = v.clamp(IDLE_POLL_RANGE.0, IDLE_POLL_RANGE.1);
            }
            SettingsChange::ThresholdEnabled(v) => s.threshold_enabled = v,
            SettingsChange::ThresholdPercent(v) => s.threshold_percent = v.clamp(50.0, 99.0),
            SettingsChange::AlertProvider(p, on) => set_membership(&mut s.alert_providers, p, on),
            SettingsChange::ProviderEnabled(p, on) => {
                let mut next = s.enabled_providers.clone();
                set_membership(&mut next, p, on);
                // An empty set reverts to the old value (spec 01 §2).
                if !next.is_empty() {
                    s.enabled_providers = next;
                    if !on && s.selected_provider == p {
                        s.selected_provider = ProviderId::Overview;
                    }
                }
            }
            SettingsChange::MoveProvider { moving, target } => {
                let mut order = s.provider_order.to_vec();
                let from = order.iter().position(|p| *p == moving);
                let to = order.iter().position(|p| *p == target);
                if let (Some(from), Some(to)) = (from, to) {
                    let item = order.remove(from);
                    order.insert(to, item);
                    s.provider_order = ProviderId::normalized_order(&order);
                }
            }
            SettingsChange::ProductVisible(c, on) => {
                s.visible_products.retain(|x| *x != c);
                if on {
                    s.visible_products.push(c);
                    s.visible_products.sort();
                }
            }
        }
    }
}

/// A call [`RecordingHost`] received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCall {
    /// `set_launch_at_login`.
    LaunchAtLogin(bool),
    /// `set_desktop_menu`.
    DesktopMenu(bool),
    /// `update_primary_action`.
    UpdateAction,
    /// `quit`.
    Quit,
    /// `sign_in`.
    SignIn(ProviderId),
    /// `sign_out`.
    SignOut(ProviderId),
    /// `save_openrouter_key` with the key received.
    SaveOpenRouterKey(String),
    /// `sign_out_openrouter`.
    SignOutOpenRouter,
    /// `export_history`.
    Export(ExportFormat, PathBuf),
    /// `clear_history`.
    ClearHistory,
}

/// [`SettingsHost`] that records calls and serves a scripted [`HostStatus`].
#[derive(Debug, Default)]
pub struct RecordingHost {
    /// Status returned by `status()`; tests mutate it between calls.
    pub status: Mutex<HostStatus>,
    /// Calls in order.
    pub calls: Mutex<Vec<HostCall>>,
    /// Error returned by fallible actions when set.
    pub fail_with: Mutex<Option<String>>,
}

impl RecordingHost {
    /// Calls so far.
    pub fn calls(&self) -> Vec<HostCall> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn record(&self, c: HostCall) {
        self.calls.lock().expect("calls lock").push(c);
    }

    fn result(&self) -> Result<(), String> {
        self.fail_with
            .lock()
            .expect("fail lock")
            .clone()
            .map_or(Ok(()), Err)
    }
}

impl SettingsHost for RecordingHost {
    fn status(&self) -> HostStatus {
        self.status.lock().expect("status lock").clone()
    }
    fn set_launch_at_login(&self, on: bool) -> Result<(), String> {
        self.record(HostCall::LaunchAtLogin(on));
        let r = self.result();
        if r.is_ok() {
            self.status.lock().expect("status lock").launch_at_login = on;
        }
        r
    }
    fn set_desktop_menu(&self, on: bool) -> Result<(), String> {
        self.record(HostCall::DesktopMenu(on));
        let r = self.result();
        if r.is_ok() {
            self.status
                .lock()
                .expect("status lock")
                .desktop_menu_enabled = on;
        }
        r
    }
    fn update_primary_action(&self) {
        self.record(HostCall::UpdateAction);
    }
    fn quit(&self) {
        self.record(HostCall::Quit);
    }
    fn sign_in(&self, provider: ProviderId) {
        self.record(HostCall::SignIn(provider));
    }
    fn sign_out(&self, provider: ProviderId) {
        self.record(HostCall::SignOut(provider));
    }
    fn save_openrouter_key(&self, key: &str) -> Result<(), String> {
        self.record(HostCall::SaveOpenRouterKey(key.to_owned()));
        self.result()
    }
    fn sign_out_openrouter(&self) {
        self.record(HostCall::SignOutOpenRouter);
    }
    fn export_history(&self, format: ExportFormat, path: &Path) -> Result<(), String> {
        self.record(HostCall::Export(format, path.to_path_buf()));
        self.result()
    }
    fn clear_history(&self) {
        self.record(HostCall::ClearHistory);
    }
}

/// [`SaveDialog`] that answers with a fixed path (or cancels).
#[derive(Debug, Default)]
pub struct FixedSaveDialog {
    /// Path to return; `None` simulates Cancel.
    pub answer: Option<PathBuf>,
    /// Default names the dialog was opened with.
    pub requested: Mutex<Vec<String>>,
}

impl SaveDialog for FixedSaveDialog {
    fn pick_save_path(&self, default_name: &str, _format: ExportFormat) -> Option<PathBuf> {
        self.requested
            .lock()
            .expect("requested lock")
            .push(default_name.to_owned());
        self.answer.clone()
    }
}
