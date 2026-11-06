//! Typed inputs and outputs of the Settings window: the values the window
//! edits ([`SettingsSnapshot`], [`SettingsChange`]), the traits the app
//! implements to back it, and the small enums the UI speaks in.

use std::path::{Path, PathBuf};

use tokenmon_core::ProviderId;

/// Poll-interval bounds for "While menu is open" (seconds, step 15).
pub const ACTIVE_POLL_RANGE: (u32, u32, u32) = (15, 300, 15);
/// Poll-interval bounds for "While idle" (seconds, step 60).
pub const IDLE_POLL_RANGE: (u32, u32, u32) = (60, 3600, 60);
/// Alert threshold slider bounds in percent.
pub const THRESHOLD_RANGE: (u32, u32) = (50, 99);

/// A Grok product category that can be shown in the breakdown (spec 01 §2.1 Data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProductCategory {
    /// Grok Build.
    Build,
    /// API.
    Api,
    /// Chat.
    Chat,
    /// Imagine.
    Imagine,
    /// Voice.
    Voice,
    /// Other.
    Other,
}

impl ProductCategory {
    /// Every category in the order the Data tab lists them.
    pub const ALL: [Self; 6] = [
        Self::Build,
        Self::Api,
        Self::Chat,
        Self::Imagine,
        Self::Voice,
        Self::Other,
    ];

    /// Persisted id (`visibleProductIDs` entry).
    pub const fn id(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Api => "api",
            Self::Chat => "chat",
            Self::Imagine => "imagine",
            Self::Voice => "voice",
            Self::Other => "other",
        }
    }

    /// Label shown on the toggle.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Build => "Grok Build",
            Self::Api => "API",
            Self::Chat => "Chat",
            Self::Imagine => "Imagine",
            Self::Voice => "Voice",
            Self::Other => "Other",
        }
    }

    /// Parses a persisted id (case-insensitive); `None` when unknown.
    pub fn from_id(id: &str) -> Option<Self> {
        let lower = id.to_lowercase();
        Self::ALL.into_iter().find(|c| c.id() == lower)
    }
}

/// File format of the SuperGrok history export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    /// Comma-separated values.
    Csv,
    /// JSON document.
    Json,
}

impl ExportFormat {
    /// File extension without the dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }

    /// Default name offered by the save dialog.
    pub const fn default_file_name(self) -> &'static str {
        match self {
            Self::Csv => "grok-usage.csv",
            Self::Json => "grok-usage.json",
        }
    }

    /// Parses `"csv"` / `"json"`.
    pub fn from_extension(s: &str) -> Option<Self> {
        match s {
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Every value the Settings window edits, as the settings store reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsSnapshot {
    /// `showCategoriesInMenuBar`.
    pub show_categories_in_menu_bar: bool,
    /// `menuBarGraphProviders`.
    pub menu_bar_graph_providers: Vec<ProviderId>,
    /// `showSelectedProviderInMenuBar`.
    pub show_selected_provider_only: bool,
    /// `checksForUpdates`.
    pub checks_for_updates: bool,
    /// `activePollSeconds`.
    pub active_poll_seconds: u32,
    /// `idlePollSeconds`.
    pub idle_poll_seconds: u32,
    /// `thresholdEnabled`.
    pub threshold_enabled: bool,
    /// `thresholdPercent`.
    pub threshold_percent: f64,
    /// `alertProviders`.
    pub alert_providers: Vec<ProviderId>,
    /// `enabledProviderIDs`.
    pub enabled_providers: Vec<ProviderId>,
    /// `orderedUsageProviders` (already normalized).
    pub provider_order: [ProviderId; 7],
    /// `visibleProductIDs`.
    pub visible_products: Vec<ProductCategory>,
    /// `selectedProvider` (Overview switches the refresh status to per-provider lines).
    pub selected_provider: ProviderId,
}

impl Default for SettingsSnapshot {
    /// Spec 01 §2 defaults.
    fn default() -> Self {
        Self {
            show_categories_in_menu_bar: true,
            menu_bar_graph_providers: vec![ProviderId::Grok],
            show_selected_provider_only: false,
            checks_for_updates: true,
            active_poll_seconds: 60,
            idle_poll_seconds: 300,
            threshold_enabled: false,
            threshold_percent: 80.0,
            alert_providers: ProviderId::USAGE_PROVIDERS.to_vec(),
            enabled_providers: ProviderId::USAGE_PROVIDERS.to_vec(),
            provider_order: ProviderId::USAGE_PROVIDERS,
            visible_products: ProductCategory::ALL.to_vec(),
            selected_provider: ProviderId::Grok,
        }
    }
}

impl SettingsSnapshot {
    /// `visibleUsageProviders`: ordered providers that are enabled.
    pub fn visible_usage_providers(&self) -> Vec<ProviderId> {
        self.provider_order
            .iter()
            .copied()
            .filter(|p| self.enabled_providers.contains(p))
            .collect()
    }
}

/// One user edit. The backend applies clamps and persistence (spec 01 §2).
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsChange {
    /// "Grok categories".
    ShowCategoriesInMenuBar(bool),
    /// "<Name> bar graph".
    MenuBarGraph(ProviderId, bool),
    /// "Show selected provider only".
    ShowSelectedProviderOnly(bool),
    /// "Check automatically".
    ChecksForUpdates(bool),
    /// "While menu is open" seconds (already within range).
    ActivePollSeconds(u32),
    /// "While idle" seconds (already within range).
    IdlePollSeconds(u32),
    /// "Notify when usage exceeds threshold".
    ThresholdEnabled(bool),
    /// Alert slider percent (already within range).
    ThresholdPercent(f64),
    /// Per-provider alert toggle.
    AlertProvider(ProviderId, bool),
    /// Per-provider on/off; the view model never sends a change that would
    /// disable the last enabled provider.
    ProviderEnabled(ProviderId, bool),
    /// `moveProvider(moving, target)`: `moving` ends at `target`'s index.
    MoveProvider {
        /// Provider being dragged.
        moving: ProviderId,
        /// Provider whose position it takes.
        target: ProviderId,
    },
    /// Grok product category toggle.
    ProductVisible(ProductCategory, bool),
}

/// Adapter over `AppSettings` (ws-settings). The app implements it; tests use
/// [`crate::settings::fake::InMemorySettings`].
pub trait SettingsBackend: Send + Sync {
    /// Current values.
    fn snapshot(&self) -> SettingsSnapshot;
    /// Applies and persists one edit.
    fn apply(&self, change: SettingsChange);
}

/// State owned by other subsystems that the window displays.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostStatus {
    /// Autostart Run-key state (ws-shell).
    pub launch_at_login: bool,
    /// Whether the desktop right-click verb is installed (ws-shell).
    pub desktop_menu_enabled: bool,
    /// App version, `None` renders "unknown".
    pub app_version: Option<String>,
    /// Title of the update button (`updateChecker.actionTitle`).
    pub update_action_title: String,
    /// Whether the update button is enabled (`canAct`).
    pub update_can_act: bool,
    /// Update checker `statusMessage`.
    pub update_status_message: Option<String>,
    /// Refresh status block.
    pub refresh: RefreshStatus,
    /// Account rows for Grok, Claude, ChatGPT, Cursor, OpenCode (any order; the
    /// window orders them).
    pub accounts: Vec<AccountStatus>,
    /// OpenRouter connection.
    pub openrouter: OpenRouterStatus,
    /// `history.storeFailed`.
    pub history_store_failed: bool,
}

/// What the "Refresh" section shows. Times arrive preformatted (local time zone
/// is the app's concern).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RefreshStatus {
    /// No refresh yet: nothing is shown.
    #[default]
    None,
    /// Overview selected: `(provider, "<date abbreviated, time shortened>")`.
    PerProvider(Vec<(ProviderId, String)>),
    /// A provider is selected.
    Single {
        /// Preformatted time of the last refresh.
        last_refresh: String,
        /// The poller's `lastError`.
        last_error: Option<String>,
    },
}

/// One signed-in provider account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStatus {
    /// Provider (Grok, Claude, ChatGPT, Cursor or OpenCode).
    pub provider: ProviderId,
    /// Whether a session is stored.
    pub signed_in: bool,
    /// Account email when known.
    pub email: Option<String>,
    /// `lastAuthError`.
    pub last_error: Option<String>,
}

/// OpenRouter API-key connection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpenRouterStatus {
    /// A key is stored.
    pub connected: bool,
    /// `lastAuthError`.
    pub last_error: Option<String>,
}

/// Everything the window asks of the rest of the app. ws-app implements it by
/// wiring ws-shell, the update checker, ws-webauth, the secrets vault and the
/// Grok history.
pub trait SettingsHost: Send + Sync {
    /// Current external state.
    fn status(&self) -> HostStatus;
    /// Writes or removes the autostart entry.
    fn set_launch_at_login(&self, on: bool) -> Result<(), String>;
    /// Installs or removes the desktop right-click verb.
    fn set_desktop_menu(&self, on: bool) -> Result<(), String>;
    /// Runs the update checker's primary action.
    fn update_primary_action(&self);
    /// Quits the app.
    fn quit(&self);
    /// Opens the sign-in window (also "Re-authenticate…").
    fn sign_in(&self, provider: ProviderId);
    /// Signs the provider out.
    fn sign_out(&self, provider: ProviderId);
    /// Validates and stores an OpenRouter key, then refreshes. The error is the
    /// message to show in red.
    fn save_openrouter_key(&self, key: &str) -> Result<(), String>;
    /// Removes the OpenRouter key.
    fn sign_out_openrouter(&self);
    /// Writes the Grok history export to `path`.
    fn export_history(&self, format: ExportFormat, path: &Path) -> Result<(), String>;
    /// `history.clear()`.
    fn clear_history(&self);
}

/// Native save dialog (rfd on Windows; fakes in tests).
pub trait SaveDialog: Send + Sync {
    /// Asks for a destination; `None` when cancelled.
    fn pick_save_path(&self, default_name: &str, format: ExportFormat) -> Option<PathBuf>;
}

/// [`SaveDialog`] backed by the platform's native dialog via `rfd`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeSaveDialog;

impl SaveDialog for NativeSaveDialog {
    fn pick_save_path(&self, default_name: &str, format: ExportFormat) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_file_name(default_name)
            .add_filter(format.extension().to_uppercase(), &[format.extension()])
            .save_file()
    }
}
