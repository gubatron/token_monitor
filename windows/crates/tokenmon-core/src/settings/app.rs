//! [`AppSettings`]: the live, persisted, observable settings handle.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::watch;

use super::file::SettingsFile;
use super::model::{
    GrokProduct, InvalidEnabledProviders, LEGACY_GRAPH_FLAGS, PanelSection, Settings, SettingsKey,
};
use crate::provider::ProviderId;

/// The app's settings: a [`Settings`] snapshot behind a watch channel, written
/// through to a [`SettingsFile`].
///
/// Readers call [`snapshot`](Self::snapshot) or [`subscribe`](Self::subscribe) and
/// compare the fields they care about (the Windows replacement for
/// `ObservationLoop`). A write that does not change the value does not notify.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use tokenmon_core::settings::{AppSettings, SettingsFile};
///
/// let settings = AppSettings::new(Arc::new(SettingsFile::in_memory()));
/// let mut rx = settings.subscribe();
/// settings.set_active_poll_seconds(10);
/// assert_eq!(rx.borrow_and_update().active_poll_seconds, 15);
/// ```
#[derive(Debug)]
pub struct AppSettings {
    file: Arc<SettingsFile>,
    tx: watch::Sender<Settings>,
}

impl AppSettings {
    /// Loads settings from `file`, running the legacy per-provider graph-flag
    /// migration when `menuBarGraphProviders` is absent.
    pub fn new(file: Arc<SettingsFile>) -> Self {
        let map = file.snapshot();
        let settings = Settings::from_map(&map);
        if !map.contains_key(SettingsKey::MenuBarGraphProviders.as_str()) {
            let removals = LEGACY_GRAPH_FLAGS
                .iter()
                .map(|(_, key, _)| ((*key).to_owned(), None));
            let save = (
                SettingsKey::MenuBarGraphProviders.as_str().to_owned(),
                Some(SettingsKey::MenuBarGraphProviders.encode(&settings)),
            );
            file.apply(removals.chain([save]));
        }
        let (tx, _rx) = watch::channel(settings);
        Self { file, tx }
    }

    /// Opens (or creates on first write) the settings file at `path`.
    pub fn open(path: impl AsRef<Path>) -> Self {
        Self::new(Arc::new(SettingsFile::open(path.as_ref())))
    }

    /// The shared backing file, for the threshold notifier's records.
    pub fn file(&self) -> Arc<SettingsFile> {
        Arc::clone(&self.file)
    }

    /// The current settings.
    pub fn snapshot(&self) -> Settings {
        self.tx.borrow().clone()
    }

    /// Subscribes to changes; each notification carries the new full snapshot.
    pub fn subscribe(&self) -> watch::Receiver<Settings> {
        self.tx.subscribe()
    }

    /// Applies `f`, persists the keys whose encoded value changed (plus `force`),
    /// and notifies subscribers if anything changed.
    fn mutate(&self, force: &[SettingsKey], f: impl FnOnce(&Settings) -> Settings) {
        self.tx.send_if_modified(|current| {
            let next = f(current);
            let edits: Vec<(String, Option<Value>)> = SettingsKey::ALL
                .iter()
                .filter(|k| force.contains(k) || k.encode(&next) != k.encode(current))
                .map(|k| (k.as_str().to_owned(), Some(k.encode(&next))))
                .collect();
            if !edits.is_empty() {
                self.file.apply(edits);
            }
            let changed = next != *current;
            *current = next;
            changed
        });
    }

    /// Sets `showCategoriesInMenuBar`.
    pub fn set_show_categories_in_menu_bar(&self, on: bool) {
        self.mutate(&[], |s| Settings {
            show_categories_in_menu_bar: on,
            ..s.clone()
        });
    }

    /// Sets `showSelectedProviderInMenuBar`.
    pub fn set_show_selected_provider_in_menu_bar(&self, on: bool) {
        self.mutate(&[], |s| Settings {
            show_selected_provider_in_menu_bar: on,
            ..s.clone()
        });
    }

    /// Sets `checksForUpdates`.
    pub fn set_checks_for_updates(&self, on: bool) {
        self.mutate(&[], |s| Settings {
            checks_for_updates: on,
            ..s.clone()
        });
    }

    /// Sets the active poll interval (clamped 15..=300) and always writes it.
    pub fn set_active_poll_seconds(&self, seconds: u32) {
        self.mutate(&[SettingsKey::ActivePollSeconds], |s| {
            s.clone().with_active_poll_seconds(seconds)
        });
    }

    /// Sets the idle poll interval (clamped 60..=3600) and always writes it.
    pub fn set_idle_poll_seconds(&self, seconds: u32) {
        self.mutate(&[SettingsKey::IdlePollSeconds], |s| {
            s.clone().with_idle_poll_seconds(seconds)
        });
    }

    /// Turns the usage alert on or off (always written).
    pub fn set_threshold_enabled(&self, on: bool) {
        self.mutate(&[SettingsKey::ThresholdEnabled], |s| Settings {
            threshold_enabled: on,
            ..s.clone()
        });
    }

    /// Sets the threshold (clamped 50..=99) and always writes it.
    pub fn set_threshold_percent(&self, percent: f64) {
        self.mutate(&[SettingsKey::ThresholdPercent], |s| {
            s.clone().with_threshold_percent(percent)
        });
    }

    /// Sets the providers the alert watches.
    pub fn set_alert_providers(&self, providers: &BTreeSet<ProviderId>) {
        self.mutate(&[], |s| s.clone().with_alert_providers(providers));
    }

    /// Selects a dropdown tab.
    pub fn set_selected_provider(&self, p: ProviderId) {
        self.mutate(&[], |s| s.clone().with_selected_provider(p));
    }

    /// Sets the enabled providers. An empty or non-usage set is rejected and the
    /// previous value kept. Disabling the selected tab selects Overview.
    pub fn set_enabled_providers(
        &self,
        enabled: &BTreeSet<ProviderId>,
    ) -> Result<(), InvalidEnabledProviders> {
        // Validation depends only on `enabled`, so checking against defaults is exact.
        Settings::default().with_enabled_providers(enabled)?;
        self.mutate(&[], |s| {
            s.clone()
                .with_enabled_providers(enabled)
                .unwrap_or_else(|_| s.clone())
        });
        Ok(())
    }

    /// Sets the provider order (normalized before it is stored).
    pub fn set_provider_order(&self, raw: &[ProviderId]) {
        self.mutate(&[], |s| s.clone().with_provider_order(raw));
    }

    /// Moves `moving` onto `target`'s slot.
    pub fn move_provider(&self, moving: ProviderId, target: ProviderId) {
        self.mutate(&[], |s| s.clone().move_provider(moving, target));
    }

    /// Pins or unpins a provider's menu-bar bar graph.
    pub fn set_menu_bar_graph(&self, p: ProviderId, shown: bool) {
        self.mutate(&[], |s| s.clone().with_menu_bar_graph(p, shown));
    }

    /// Expands or collapses a dropdown section.
    pub fn set_expanded(&self, expanded: bool, section: PanelSection) {
        self.mutate(&[], |s| s.clone().with_expanded(section, expanded));
    }

    /// Sets the visible Grok products (always written; empty is allowed).
    pub fn set_visible_products(&self, products: &BTreeSet<GrokProduct>) {
        self.mutate(&[SettingsKey::VisibleProductIds], |s| {
            s.clone().with_visible_products(products.clone())
        });
    }
}
