//! The Settings view model: turns user intents into backend/host calls and
//! rebuilds the display model after each one.

use std::sync::{Arc, Mutex};

use tokenmon_core::ProviderId;

use super::model::{
    ACTIVE_POLL_RANGE, ExportFormat, IDLE_POLL_RANGE, ProductCategory, SaveDialog, SettingsBackend,
    SettingsChange, SettingsHost, THRESHOLD_RANGE,
};
use super::screen::{GROK_CATEGORIES_ID, LocalState, SettingsScreen, build_screen};

/// Direction of a stepper press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Decrease by one step.
    Down,
    /// Increase by one step.
    Up,
}

/// Plain-Rust view model of the Settings window (no Slint types).
pub struct SettingsViewModel {
    backend: Arc<dyn SettingsBackend>,
    host: Arc<dyn SettingsHost>,
    dialog: Arc<dyn SaveDialog>,
    local: Mutex<LocalState>,
}

/// Clamped stepper move: `current` +/- `step` within `[min, max]`.
pub fn stepped(current: u32, dir: Step, (min, max, step): (u32, u32, u32)) -> u32 {
    let next = match dir {
        Step::Down => current.saturating_sub(step),
        Step::Up => current.saturating_add(step),
    };
    next.clamp(min, max)
}

impl SettingsViewModel {
    /// Creates a view model over the given adapters.
    pub fn new(
        backend: Arc<dyn SettingsBackend>,
        host: Arc<dyn SettingsHost>,
        dialog: Arc<dyn SaveDialog>,
    ) -> Self {
        Self {
            backend,
            host,
            dialog,
            local: Mutex::new(LocalState::default()),
        }
    }

    fn local(&self) -> std::sync::MutexGuard<'_, LocalState> {
        self.local
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Current display model.
    pub fn screen(&self) -> SettingsScreen {
        build_screen(&self.backend.snapshot(), &self.host.status(), &self.local())
    }

    /// Toggles "Launch at Login". The window re-reads the real state afterwards,
    /// so a failed write reverts the switch (spec 01 §1.x launch-at-login).
    pub fn set_launch_at_login(&self, on: bool) {
        let result = self.host.set_launch_at_login(on);
        self.local().shell_error = result.err();
    }

    /// Toggles the desktop right-click verb.
    pub fn set_desktop_menu(&self, on: bool) {
        let result = self.host.set_desktop_menu(on);
        self.local().shell_error = result.err();
    }

    /// "Check automatically".
    pub fn set_checks_for_updates(&self, on: bool) {
        self.backend.apply(SettingsChange::ChecksForUpdates(on));
    }

    /// The update button.
    pub fn update_primary_action(&self) {
        self.host.update_primary_action();
    }

    /// "While menu is open" stepper.
    pub fn step_active(&self, dir: Step) {
        let cur = self.backend.snapshot().active_poll_seconds;
        self.backend
            .apply(SettingsChange::ActivePollSeconds(stepped(
                cur,
                dir,
                ACTIVE_POLL_RANGE,
            )));
    }

    /// "While idle" stepper.
    pub fn step_idle(&self, dir: Step) {
        let cur = self.backend.snapshot().idle_poll_seconds;
        self.backend.apply(SettingsChange::IdlePollSeconds(stepped(
            cur,
            dir,
            IDLE_POLL_RANGE,
        )));
    }

    /// Alert master switch.
    pub fn set_threshold_enabled(&self, on: bool) {
        self.backend.apply(SettingsChange::ThresholdEnabled(on));
    }

    /// Alert slider; rounded to whole percent and clamped to 50-99.
    pub fn set_threshold_percent(&self, value: f64) {
        let (lo, hi) = THRESHOLD_RANGE;
        let p = value.round().clamp(f64::from(lo), f64::from(hi));
        self.backend.apply(SettingsChange::ThresholdPercent(p));
    }

    /// Per-provider alert switch; unknown ids are ignored.
    pub fn set_alert_provider(&self, id: &str, on: bool) {
        if let Ok(p) = id.parse::<ProviderId>() {
            self.backend.apply(SettingsChange::AlertProvider(p, on));
        }
    }

    /// Provider on/off. Refuses to disable the only enabled provider.
    pub fn set_provider_enabled(&self, id: &str, on: bool) {
        let Ok(p) = id.parse::<ProviderId>() else {
            return;
        };
        let snap = self.backend.snapshot();
        if !on && snap.enabled_providers.len() <= 1 && snap.enabled_providers.contains(&p) {
            return;
        }
        self.backend.apply(SettingsChange::ProviderEnabled(p, on));
    }

    /// Drag-reorder: moves `id` to list position `target_position`.
    pub fn move_provider(&self, id: &str, target_position: i32) {
        let Ok(moving) = id.parse::<ProviderId>() else {
            return;
        };
        let order = self.backend.snapshot().provider_order;
        let Some(idx) = usize::try_from(target_position)
            .ok()
            .filter(|i| *i < order.len())
        else {
            return;
        };
        let target = order[idx];
        if target != moving {
            self.backend
                .apply(SettingsChange::MoveProvider { moving, target });
        }
    }

    /// "Show selected provider only".
    pub fn set_show_selected_only(&self, on: bool) {
        self.backend
            .apply(SettingsChange::ShowSelectedProviderOnly(on));
    }

    /// A Menu Bar switch (a provider id or the "Grok categories" row).
    pub fn set_menu_bar_graph(&self, id: &str, on: bool) {
        if id == GROK_CATEGORIES_ID {
            self.backend
                .apply(SettingsChange::ShowCategoriesInMenuBar(on));
        } else if let Ok(p) = id.parse::<ProviderId>()
            && p.supports_menu_bar_graph()
        {
            self.backend.apply(SettingsChange::MenuBarGraph(p, on));
        }
    }

    /// Account "Sign In…" / "Re-authenticate…".
    pub fn sign_in(&self, id: &str) {
        if let Ok(p) = id.parse::<ProviderId>()
            && p.has_sign_in_window()
        {
            self.host.sign_in(p);
        }
    }

    /// Account "Sign Out".
    pub fn sign_out(&self, id: &str) {
        if let Ok(p) = id.parse::<ProviderId>() {
            self.host.sign_out(p);
        }
    }

    /// OpenRouter "Save API Key". An empty (trimmed) draft is ignored (the button
    /// is disabled); returns true when the key was accepted so the UI clears the
    /// draft field.
    pub fn save_openrouter_key(&self, draft: &str) -> bool {
        let key = draft.trim();
        if key.is_empty() {
            return false;
        }
        match self.host.save_openrouter_key(key) {
            Ok(()) => {
                self.local().openrouter_error = None;
                true
            }
            Err(msg) => {
                self.local().openrouter_error = Some(msg);
                false
            }
        }
    }

    /// OpenRouter "Sign Out" (the UI also clears its draft).
    pub fn sign_out_openrouter(&self) {
        self.local().openrouter_error = None;
        self.host.sign_out_openrouter();
    }

    /// Grok category switch.
    pub fn set_product_visible(&self, id: &str, on: bool) {
        if let Some(c) = ProductCategory::from_id(id) {
            self.backend.apply(SettingsChange::ProductVisible(c, on));
        }
    }

    /// "CSV…" / "JSON…": clears the old error, asks for a path and exports.
    /// A cancelled dialog changes nothing.
    pub fn export_history(&self, format: ExportFormat) {
        self.local().export_error = None;
        let Some(path) = self
            .dialog
            .pick_save_path(format.default_file_name(), format)
        else {
            return;
        };
        if let Err(msg) = self.host.export_history(format, &path) {
            self.local().export_error = Some(msg);
        }
    }

    /// "Clear Local History".
    pub fn clear_history(&self) {
        self.host.clear_history();
    }

    /// "Quit TokenMon".
    pub fn quit(&self) {
        self.host.quit();
    }
}
