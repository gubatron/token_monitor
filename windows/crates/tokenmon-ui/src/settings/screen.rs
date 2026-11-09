//! Display models: pure functions from (settings, host status, UI-local
//! state) to exactly the strings and flags each tab shows (spec 01 §2.1).

use tokenmon_core::ProviderId;

use super::model::{AccountStatus, HostStatus, ProductCategory, RefreshStatus, SettingsSnapshot};

/// Footer of the Updates section.
pub const UPDATES_FOOTER: &str = "Checks GitHub for a newer release. Update downloads the release and replaces this copy of TokenMon.";
/// Footer of the alert-provider list.
pub const ALERTS_FOOTER: &str = "Providers the alert watches keep refreshing in the background.";
/// Footer of the Providers section.
pub const PROVIDERS_FOOTER: &str = "Turn a provider on to show its tab in the menu. Drag a row to set the order of tabs, Overview cards, and menu bar graphs.";
/// Menu bar footer when "Show selected provider only" is on.
pub const MENU_BAR_FOOTER_ON: &str =
    "The menu bar shows only the active provider's icon, percentage, and usage bar.";
/// Menu bar footer when it is off.
pub const MENU_BAR_FOOTER_OFF: &str = "Choose which graphs stay pinned in the menu bar.";
/// Footer of the Accounts section.
pub const ACCOUNTS_FOOTER: &str = "Grokbot uses your Cursor account.";
/// Privacy note shown above the Accounts list (issue #82, decision #38).
pub const ACCOUNTS_PRIVACY_NOTE: &str = "TokenMon has no servers and never reads or stores your passwords. When you sign in, the provider's own page handles your login; TokenMon keeps only the session it needs, always encrypted with your Windows account (Windows Data Protection) on this PC in %LOCALAPPDATA%\\TokenMon. It is sent only to the provider it belongs to, and signing out deletes it.";
/// Footer of the SuperGrok Categories section.
pub const CATEGORIES_FOOTER: &str = "Categories shown in the Grok breakdown.";
/// Footer of the SuperGrok History section.
pub const HISTORY_FOOTER: &str = "Clearing history does not reset your SuperGrok weekly pool.";
/// Shown in red when the history store could not be opened.
pub const HISTORY_STORE_FAILED: &str = "History storage is unavailable \u{2014} usage is kept for this session only and will not survive a relaunch.";
/// Providers that have an Accounts row, in display order.
pub const ACCOUNT_PROVIDERS: [ProviderId; 5] = [
    ProviderId::Grok,
    ProviderId::Claude,
    ProviderId::ChatGpt,
    ProviderId::Cursor,
    ProviderId::OpenCode,
];

/// A tab of the Settings window, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    /// General.
    #[default]
    General,
    /// Providers.
    Providers,
    /// Accounts ("sign in to providers").
    Accounts,
    /// Data.
    Data,
}

impl SettingsTab {
    /// Position in the tab bar.
    pub const fn index(self) -> i32 {
        match self {
            Self::General => 0,
            Self::Providers => 1,
            Self::Accounts => 2,
            Self::Data => 3,
        }
    }

    /// Tab at a tab-bar position; out-of-range falls back to General.
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Providers,
            2 => Self::Accounts,
            3 => Self::Data,
            _ => Self::General,
        }
    }
}

/// True when any cookie provider has a session or OpenRouter has a key.
pub fn any_signed_in(host: &HostStatus) -> bool {
    host.openrouter.connected || host.accounts.iter().any(|a| a.signed_in)
}

/// Tab to show when Settings opens: first run (nothing signed in) goes to
/// Accounts so the user can sign in; otherwise the last tab is kept.
pub const fn tab_on_open(any_signed_in: bool, last: SettingsTab) -> SettingsTab {
    if any_signed_in {
        last
    } else {
        SettingsTab::Accounts
    }
}

/// A labelled switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleRow {
    /// Stable id the UI sends back (provider or category id).
    pub id: String,
    /// Visible label.
    pub label: String,
    /// Switch state.
    pub on: bool,
    /// False greys the switch out.
    pub enabled: bool,
}

/// One draggable row of the Providers list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRow {
    /// Provider id string.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Enabled switch.
    pub enabled: bool,
    /// False when this is the only enabled provider.
    pub toggle_enabled: bool,
    /// Index in the current order; rows keep a fixed identity so a drag animates.
    pub position: i32,
    /// Accent color as 0xRRGGBB (stand-in for the 16 px logo until ws-ui-kit assets land).
    pub accent_rgb: u32,
}

/// One Accounts row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    /// Provider id string.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Email, "Signed in" or "Not signed in".
    pub subtitle: String,
    /// Signed in: show "Re-authenticate…" and "Sign Out".
    pub signed_in: bool,
    /// Red caption.
    pub error: String,
    /// Accent as 0xRRGGBB.
    pub accent_rgb: u32,
}

/// General tab.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneralScreen {
    /// "Launch at Login".
    pub launch_at_login: bool,
    /// "Show TokenMon in the desktop right-click menu".
    pub desktop_menu: bool,
    /// "Check automatically".
    pub checks_for_updates: bool,
    /// "Version <v>".
    pub version_text: String,
    /// Update button title.
    pub update_action_title: String,
    /// Update button enabled.
    pub update_can_act: bool,
    /// Caption under the version row (empty = hidden).
    pub update_status_message: String,
    /// "<n>s" for the menu-open interval.
    pub active_text: String,
    /// The `-` stepper button is usable.
    pub active_can_decrement: bool,
    /// The `+` stepper button is usable.
    pub active_can_increment: bool,
    /// "<n>s" for the idle interval.
    pub idle_text: String,
    /// The `-` stepper button is usable.
    pub idle_can_decrement: bool,
    /// The `+` stepper button is usable.
    pub idle_can_increment: bool,
    /// Plain refresh lines (empty = no refresh yet).
    pub refresh_lines: Vec<String>,
    /// Red refresh error caption (empty = hidden).
    pub refresh_error: String,
    /// Alert master switch.
    pub threshold_enabled: bool,
    /// "Alert at <n>%".
    pub threshold_text: String,
    /// Slider value.
    pub threshold_percent: f64,
    /// One switch per visible usage provider.
    pub alert_rows: Vec<ToggleRow>,
}

/// Providers tab.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvidersScreen {
    /// Fixed-identity rows in default provider order; `position` carries the order.
    pub rows: Vec<ProviderRow>,
    /// "Show selected provider only".
    pub show_selected_only: bool,
    /// Bar-graph switches (empty when [`Self::show_selected_only`]).
    pub graph_rows: Vec<ToggleRow>,
    /// Footer under the Menu Bar section.
    pub menu_bar_footer: String,
}

/// Accounts tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsScreen {
    /// Grok, Claude, ChatGPT, Cursor, OpenCode.
    pub rows: Vec<AccountRow>,
    /// OpenRouter has a stored key.
    pub openrouter_connected: bool,
    /// Red caption for OpenRouter.
    pub openrouter_error: String,
    /// Privacy note above the list ([`ACCOUNTS_PRIVACY_NOTE`]).
    pub privacy_note: String,
}

/// Data tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataScreen {
    /// One switch per Grok category.
    pub category_rows: Vec<ToggleRow>,
    /// Red export error (empty = hidden).
    pub export_error: String,
    /// Red storage-failed notice (empty = hidden).
    pub store_failed_text: String,
}

/// The whole window.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsScreen {
    /// General tab.
    pub general: GeneralScreen,
    /// Providers tab.
    pub providers: ProvidersScreen,
    /// Accounts tab.
    pub accounts: AccountsScreen,
    /// Data tab.
    pub data: DataScreen,
}

/// UI-local state that no other subsystem owns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalState {
    /// Last export failure; cleared by the next export.
    pub export_error: Option<String>,
    /// Last launch-at-login / desktop-menu failure.
    pub shell_error: Option<String>,
    /// Last OpenRouter save failure (shown with the host's `last_error`).
    pub openrouter_error: Option<String>,
}

/// `"<n>s"`.
pub fn seconds_text(seconds: u32) -> String {
    format!("{seconds}s")
}

/// `"Alert at <n>%"`.
pub fn threshold_text(percent: f64) -> String {
    format!("Alert at {}%", percent.round() as i64)
}

/// Subtitle of an account row: email, "Signed in" or "Not signed in".
pub fn account_subtitle(status: &AccountStatus) -> String {
    match (status.signed_in, status.email.as_deref()) {
        (true, Some(e)) if !e.is_empty() => e.to_owned(),
        (true, _) => "Signed in".to_owned(),
        (false, _) => "Not signed in".to_owned(),
    }
}

fn rgb_u32(p: ProviderId) -> u32 {
    let (r, g, b) = p.accent().to_rgb8();
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

fn general(s: &SettingsSnapshot, h: &HostStatus, local: &LocalState) -> GeneralScreen {
    let (refresh_lines, mut refresh_error) = match &h.refresh {
        RefreshStatus::None => (Vec::new(), None),
        RefreshStatus::PerProvider(v) => (
            v.iter()
                .map(|(p, t)| format!("{}: {t}", p.display_name()))
                .collect(),
            None,
        ),
        RefreshStatus::Single {
            last_refresh,
            last_error,
        } => (
            vec![format!("Last refresh: {last_refresh}")],
            last_error.clone(),
        ),
    };
    refresh_error = refresh_error.or_else(|| local.shell_error.clone());
    let (amin, amax, _) = super::model::ACTIVE_POLL_RANGE;
    let (imin, imax, _) = super::model::IDLE_POLL_RANGE;
    GeneralScreen {
        launch_at_login: h.launch_at_login,
        desktop_menu: h.desktop_menu_enabled,
        checks_for_updates: s.checks_for_updates,
        version_text: format!("Version {}", h.app_version.as_deref().unwrap_or("unknown")),
        update_action_title: h.update_action_title.clone(),
        update_can_act: h.update_can_act,
        update_status_message: h.update_status_message.clone().unwrap_or_default(),
        active_text: seconds_text(s.active_poll_seconds),
        active_can_decrement: s.active_poll_seconds > amin,
        active_can_increment: s.active_poll_seconds < amax,
        idle_text: seconds_text(s.idle_poll_seconds),
        idle_can_decrement: s.idle_poll_seconds > imin,
        idle_can_increment: s.idle_poll_seconds < imax,
        refresh_lines,
        refresh_error: refresh_error.unwrap_or_default(),
        threshold_enabled: s.threshold_enabled,
        threshold_text: threshold_text(s.threshold_percent),
        threshold_percent: s.threshold_percent,
        alert_rows: s
            .visible_usage_providers()
            .into_iter()
            .map(|p| ToggleRow {
                id: p.as_str().to_owned(),
                label: p.display_name().to_owned(),
                on: s.alert_providers.contains(&p),
                enabled: true,
            })
            .collect(),
    }
}

fn providers(s: &SettingsSnapshot) -> ProvidersScreen {
    let only_one = s.enabled_providers.len() <= 1;
    let rows = ProviderId::USAGE_PROVIDERS
        .iter()
        .map(|&p| {
            let enabled = s.enabled_providers.contains(&p);
            ProviderRow {
                id: p.as_str().to_owned(),
                name: p.display_name().to_owned(),
                enabled,
                toggle_enabled: !(enabled && only_one),
                position: s
                    .provider_order
                    .iter()
                    .position(|&o| o == p)
                    .and_then(|i| i32::try_from(i).ok())
                    .unwrap_or(0),
                accent_rgb: rgb_u32(p),
            }
        })
        .collect();
    let graph_rows = if s.show_selected_provider_only {
        Vec::new()
    } else {
        graph_rows(s)
    };
    ProvidersScreen {
        rows,
        show_selected_only: s.show_selected_provider_only,
        graph_rows,
        menu_bar_footer: if s.show_selected_provider_only {
            MENU_BAR_FOOTER_ON
        } else {
            MENU_BAR_FOOTER_OFF
        }
        .to_owned(),
    }
}

/// Id of the "Grok categories" row in [`ProvidersScreen::graph_rows`].
pub const GROK_CATEGORIES_ID: &str = "grok-categories";

fn graph_rows(s: &SettingsSnapshot) -> Vec<ToggleRow> {
    let mut rows = Vec::with_capacity(6);
    for p in ProviderId::MENU_BAR_GRAPH_PROVIDERS {
        rows.push(ToggleRow {
            id: p.as_str().to_owned(),
            label: format!("{} bar graph", p.display_name()),
            on: s.menu_bar_graph_providers.contains(&p),
            enabled: true,
        });
        if p == ProviderId::Grok {
            rows.push(ToggleRow {
                id: GROK_CATEGORIES_ID.to_owned(),
                label: "Grok categories".to_owned(),
                on: s.show_categories_in_menu_bar,
                enabled: true,
            });
        }
    }
    rows
}

fn accounts(h: &HostStatus, local: &LocalState) -> AccountsScreen {
    let rows = ACCOUNT_PROVIDERS
        .iter()
        .map(|&p| {
            let st = h
                .accounts
                .iter()
                .find(|a| a.provider == p)
                .cloned()
                .unwrap_or(AccountStatus {
                    provider: p,
                    signed_in: false,
                    email: None,
                    last_error: None,
                });
            AccountRow {
                id: p.as_str().to_owned(),
                name: p.display_name().to_owned(),
                subtitle: account_subtitle(&st),
                signed_in: st.signed_in,
                error: st.last_error.unwrap_or_default(),
                accent_rgb: rgb_u32(p),
            }
        })
        .collect();
    AccountsScreen {
        privacy_note: ACCOUNTS_PRIVACY_NOTE.to_owned(),
        rows,
        openrouter_connected: h.openrouter.connected,
        openrouter_error: local
            .openrouter_error
            .clone()
            .or_else(|| h.openrouter.last_error.clone())
            .unwrap_or_default(),
    }
}

fn data(s: &SettingsSnapshot, h: &HostStatus, local: &LocalState) -> DataScreen {
    DataScreen {
        category_rows: ProductCategory::ALL
            .iter()
            .map(|c| ToggleRow {
                id: c.id().to_owned(),
                label: c.display_name().to_owned(),
                on: s.visible_products.contains(c),
                enabled: true,
            })
            .collect(),
        export_error: local.export_error.clone().unwrap_or_default(),
        store_failed_text: if h.history_store_failed {
            HISTORY_STORE_FAILED.to_owned()
        } else {
            String::new()
        },
    }
}

/// Builds the full display model.
pub fn build_screen(s: &SettingsSnapshot, h: &HostStatus, local: &LocalState) -> SettingsScreen {
    SettingsScreen {
        general: general(s, h, local),
        providers: providers(s),
        accounts: accounts(h, local),
        data: data(s, h, local),
    }
}
