//! Right-click context menu model.

use tokenmon_core::ProviderId;

/// What a menu item does when chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuAction {
    /// Select this provider and open the popup.
    OpenProvider(ProviderId),
    /// Select Overview and open the popup.
    OpenOverview,
    /// Open Settings.
    Settings,
    /// Run an update check now.
    CheckForUpdates,
    /// Quit TokenMon.
    Quit,
}

impl MenuAction {
    /// Stable menu-item id (Win32 command ids are numeric; the tray crate
    /// maps this back with [`MenuAction::from_id`]).
    pub fn id(self) -> u32 {
        match self {
            Self::OpenOverview => 1,
            Self::Settings => 2,
            Self::CheckForUpdates => 3,
            Self::Quit => 4,
            Self::OpenProvider(p) => {
                100 + ProviderId::USAGE_PROVIDERS
                    .iter()
                    .position(|q| *q == p)
                    .unwrap_or(0) as u32
            }
        }
    }

    /// Inverse of [`MenuAction::id`].
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            1 => Some(Self::OpenOverview),
            2 => Some(Self::Settings),
            3 => Some(Self::CheckForUpdates),
            4 => Some(Self::Quit),
            100..=106 => Some(Self::OpenProvider(
                ProviderId::USAGE_PROVIDERS[(id - 100) as usize],
            )),
            _ => None,
        }
    }
}

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A clickable item.
    Item {
        /// Text shown.
        label: String,
        /// Action on click.
        action: MenuAction,
    },
    /// A separator line.
    Separator,
}

/// Inputs to [`build_menu`].
#[derive(Debug, Clone, Copy)]
pub struct MenuInputs<'a> {
    /// User provider order (normalized internally).
    pub provider_order: &'a [ProviderId],
    /// Enabled providers; disabled ones get no "Open" item.
    pub enabled: &'a [ProviderId],
}

fn item(label: impl Into<String>, action: MenuAction) -> MenuEntry {
    MenuEntry::Item {
        label: label.into(),
        action,
    }
}

/// Builds the menu: `Open <provider>` per enabled provider in menu order,
/// `Open Overview`, separator, `Settings…`, `Check for Updates`, separator,
/// `Quit TokenMon`.
///
/// ```
/// use tokenmon_core::ProviderId::*;
/// use tokenmon_ui::tray_icon::{build_menu, MenuInputs};
/// let m = build_menu(&MenuInputs { provider_order: &[], enabled: &[Grok] });
/// assert_eq!(m.len(), 7);
/// ```
pub fn build_menu(inputs: &MenuInputs<'_>) -> Vec<MenuEntry> {
    ProviderId::normalized_order(inputs.provider_order)
        .into_iter()
        .filter(|p| inputs.enabled.contains(p))
        .map(|p| {
            item(
                format!("Open {}", p.display_name()),
                MenuAction::OpenProvider(p),
            )
        })
        .chain([
            item("Open Overview", MenuAction::OpenOverview),
            MenuEntry::Separator,
            item("Settings\u{2026}", MenuAction::Settings),
            item("Check for Updates", MenuAction::CheckForUpdates),
            MenuEntry::Separator,
            item("Quit TokenMon", MenuAction::Quit),
        ])
        .collect()
}
