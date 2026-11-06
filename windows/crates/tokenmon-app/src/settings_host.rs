//! The Settings window's collaborators: [`AppSettingsBackend`] adapts the core
//! settings store, [`AppHost`] answers everything else the window asks of the
//! app (launch at login, the desktop verb, updates, accounts, history).
//!
//! Portable: the Windows pieces arrive as trait objects
//! ([`LoginItemService`], [`ShellVerbService`]), so the whole surface is tested
//! with the platform crate's memory registry.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, Utc};
use tokenmon_core::ProviderId;
use tokenmon_core::settings::{AppSettings, GrokProduct, Settings};
use tokenmon_platform::shell::{LoginItemService, LoginItemStatus, ShellVerbService};
use tokenmon_provider_grok::export::{self, ExportFormat as GrokFormat};
use tokenmon_ui::settings::{
    AccountStatus, ExportFormat, HostStatus, OpenRouterStatus, ProductCategory, RefreshStatus,
    SettingsBackend, SettingsChange, SettingsHost, SettingsSnapshot,
};
use tokio::runtime::Handle;

use crate::model::AppModel;
use crate::sessions::Sessions;
use crate::signin::SignInService;
use crate::sources::HistoryAccess;
use crate::updates::UpdateService;

fn category(p: GrokProduct) -> ProductCategory {
    match p {
        GrokProduct::Build => ProductCategory::Build,
        GrokProduct::Api => ProductCategory::Api,
        GrokProduct::Chat => ProductCategory::Chat,
        GrokProduct::Imagine => ProductCategory::Imagine,
        GrokProduct::Voice => ProductCategory::Voice,
        GrokProduct::Other => ProductCategory::Other,
    }
}

fn product(c: ProductCategory) -> GrokProduct {
    match c {
        ProductCategory::Build => GrokProduct::Build,
        ProductCategory::Api => GrokProduct::Api,
        ProductCategory::Chat => GrokProduct::Chat,
        ProductCategory::Imagine => GrokProduct::Imagine,
        ProductCategory::Voice => GrokProduct::Voice,
        ProductCategory::Other => GrokProduct::Other,
    }
}

fn with_member<T: Ord + Copy>(set: &BTreeSet<T>, item: T, on: bool) -> BTreeSet<T> {
    let mut next = set.clone();
    if on {
        next.insert(item);
    } else {
        next.remove(&item);
    }
    next
}

/// [`SettingsBackend`] over the core [`AppSettings`] store.
pub struct AppSettingsBackend {
    settings: Arc<AppSettings>,
}

impl AppSettingsBackend {
    /// Adapts `settings`.
    pub fn new(settings: Arc<AppSettings>) -> Self {
        Self { settings }
    }
}

fn snapshot_of(s: &Settings) -> SettingsSnapshot {
    SettingsSnapshot {
        show_categories_in_menu_bar: s.show_categories_in_menu_bar,
        menu_bar_graph_providers: s.menu_bar_graph_providers.iter().copied().collect(),
        show_selected_provider_only: s.show_selected_provider_in_menu_bar,
        checks_for_updates: s.checks_for_updates,
        active_poll_seconds: s.active_poll_seconds,
        idle_poll_seconds: s.idle_poll_seconds,
        threshold_enabled: s.threshold_enabled,
        threshold_percent: s.threshold_percent,
        alert_providers: s.alert_providers.iter().copied().collect(),
        enabled_providers: s.enabled_provider_ids.iter().copied().collect(),
        provider_order: s.provider_order,
        visible_products: s
            .visible_product_ids
            .iter()
            .copied()
            .map(category)
            .collect(),
        selected_provider: s.selected_provider,
    }
}

impl SettingsBackend for AppSettingsBackend {
    fn snapshot(&self) -> SettingsSnapshot {
        snapshot_of(&self.settings.snapshot())
    }

    fn apply(&self, change: SettingsChange) {
        let s = &self.settings;
        let current = s.snapshot();
        match change {
            SettingsChange::ShowCategoriesInMenuBar(v) => s.set_show_categories_in_menu_bar(v),
            SettingsChange::MenuBarGraph(p, on) => s.set_menu_bar_graph(p, on),
            SettingsChange::ShowSelectedProviderOnly(v) => {
                s.set_show_selected_provider_in_menu_bar(v);
            }
            SettingsChange::ChecksForUpdates(v) => s.set_checks_for_updates(v),
            SettingsChange::ActivePollSeconds(v) => s.set_active_poll_seconds(v),
            SettingsChange::IdlePollSeconds(v) => s.set_idle_poll_seconds(v),
            SettingsChange::ThresholdEnabled(v) => s.set_threshold_enabled(v),
            SettingsChange::ThresholdPercent(v) => s.set_threshold_percent(v),
            SettingsChange::AlertProvider(p, on) => {
                s.set_alert_providers(&with_member(&current.alert_providers, p, on));
            }
            SettingsChange::ProviderEnabled(p, on) => {
                // The last provider cannot be disabled; the store keeps its value.
                let _ = s.set_enabled_providers(&with_member(&current.enabled_provider_ids, p, on));
            }
            SettingsChange::MoveProvider { moving, target } => s.move_provider(moving, target),
            SettingsChange::ProductVisible(c, on) => {
                s.set_visible_products(&with_member(&current.visible_product_ids, product(c), on));
            }
        }
    }
}

/// Local time formatting for the refresh lines (`Jul 10, 2026 at 3:04 PM`).
pub fn refresh_time(at: DateTime<Utc>, tz: FixedOffset) -> String {
    at.with_timezone(&tz)
        .format("%b %-d, %Y at %-I:%M %p")
        .to_string()
}

/// How the host reads the local offset (injected so tests are hermetic).
pub type TzSource = Arc<dyn Fn() -> FixedOffset + Send + Sync>;

/// Everything [`AppHost`] is built from.
pub struct HostParts {
    /// The settings store.
    pub settings: Arc<AppSettings>,
    /// Lifecycle model (registry, refreshes).
    pub model: Arc<AppModel>,
    /// Credential sessions.
    pub sessions: Arc<Sessions>,
    /// Sign-in and sign-out.
    pub signin: Arc<SignInService>,
    /// Update checker and installer.
    pub updates: Arc<UpdateService>,
    /// Autostart service.
    pub login_item: Arc<dyn LoginItemService>,
    /// Desktop right-click verb.
    pub desktop_verb: Arc<dyn ShellVerbService>,
    /// Grok usage history.
    pub history: Arc<dyn HistoryAccess>,
    /// App version.
    pub version: String,
    /// Local offset source.
    pub tz: TzSource,
    /// Runtime for async work.
    pub handle: Handle,
    /// Quits the app.
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

/// [`SettingsHost`] backed by the app's real services.
pub struct AppHost {
    parts: HostParts,
}

impl AppHost {
    /// A host over `parts`.
    pub fn new(parts: HostParts) -> Self {
        Self { parts }
    }

    fn accounts(&self) -> Vec<AccountStatus> {
        [
            ProviderId::Grok,
            ProviderId::Claude,
            ProviderId::ChatGpt,
            ProviderId::Cursor,
            ProviderId::OpenCode,
        ]
        .into_iter()
        .map(|provider| {
            let s = self.parts.sessions.auth_state(provider);
            AccountStatus {
                provider,
                signed_in: s.is_signed_in,
                email: s.account_email,
                last_error: s.last_auth_error,
            }
        })
        .collect()
    }

    fn refresh(&self, settings: &Settings) -> RefreshStatus {
        let tz = (self.parts.tz)();
        let registry = self.parts.model.registry();
        let at = |p: ProviderId| registry.entry(p).and_then(|e| e.poller.last_refreshed_at());
        if settings.selected_provider == ProviderId::Overview {
            let lines: Vec<_> = settings
                .visible_usage_providers()
                .into_iter()
                .filter_map(|p| at(p).map(|t| (p, refresh_time(t, tz))))
                .collect();
            return if lines.is_empty() {
                RefreshStatus::None
            } else {
                RefreshStatus::PerProvider(lines)
            };
        }
        let provider = settings.selected_provider;
        match at(provider) {
            Some(t) => RefreshStatus::Single {
                last_refresh: refresh_time(t, tz),
                last_error: registry
                    .entry(provider)
                    .and_then(|e| e.poller.last_error())
                    .map(|e| e.to_string()),
            },
            None => RefreshStatus::None,
        }
    }
}

impl SettingsHost for AppHost {
    fn status(&self) -> HostStatus {
        let p = &self.parts;
        let settings = p.settings.snapshot();
        let update = p.updates.checker.state();
        let openrouter = p.sessions.auth_state(ProviderId::OpenRouter);
        HostStatus {
            launch_at_login: p
                .login_item
                .status()
                .is_ok_and(|s| s != LoginItemStatus::NotRegistered),
            desktop_menu_enabled: p.desktop_verb.is_installed().unwrap_or(false),
            app_version: Some(p.version.clone()),
            update_action_title: update.action_title(),
            update_can_act: update.can_act(),
            update_status_message: update.status_message.or(update.last_error),
            refresh: self.refresh(&settings),
            accounts: self.accounts(),
            openrouter: OpenRouterStatus {
                connected: openrouter.is_signed_in,
                last_error: openrouter.last_auth_error,
            },
            history_store_failed: p.history.store_failed(),
        }
    }

    fn set_launch_at_login(&self, on: bool) -> Result<(), String> {
        let result = if on {
            self.parts.login_item.register()
        } else {
            self.parts.login_item.unregister()
        };
        result.map_err(|e| e.to_string())
    }

    fn set_desktop_menu(&self, on: bool) -> Result<(), String> {
        let result = if on {
            self.parts.desktop_verb.install()
        } else {
            self.parts.desktop_verb.uninstall()
        };
        result.map_err(|e| e.to_string())
    }

    fn update_primary_action(&self) {
        let updates = Arc::clone(&self.parts.updates);
        self.parts.handle.spawn(async move {
            updates
                .checker
                .perform_primary_action(updates.installer.as_ref())
                .await;
        });
    }

    fn quit(&self) {
        (self.parts.quit)();
    }

    fn sign_in(&self, provider: ProviderId) {
        if let Err(e) = self.parts.signin.start(provider) {
            tracing::warn!(error = %e, %provider, "sign-in could not open");
        }
    }

    fn sign_out(&self, provider: ProviderId) {
        self.parts.signin.sign_out(provider);
    }

    fn save_openrouter_key(&self, key: &str) -> Result<(), String> {
        self.parts
            .sessions
            .openrouter
            .save_api_key(key)
            .map_err(|e| e.to_string())?;
        self.parts.model.sign_in_completed(ProviderId::OpenRouter);
        Ok(())
    }

    fn sign_out_openrouter(&self) {
        self.parts.signin.sign_out(ProviderId::OpenRouter);
    }

    fn export_history(&self, format: ExportFormat, path: &Path) -> Result<(), String> {
        let format = match format {
            ExportFormat::Csv => GrokFormat::Csv,
            ExportFormat::Json => GrokFormat::Json,
        };
        let bytes =
            export::export(&self.parts.history.snapshots(), format).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| format!("Could not write the export: {e}"))
    }

    fn clear_history(&self) {
        self.parts.history.clear();
    }
}
