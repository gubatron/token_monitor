//! Tray icon controller: decides what the notification-area icon shows and
//! only touches the OS (`NIM_MODIFY`) when something visible changed (spec 04
//! section 2).
//!
//! The portable part lives here behind [`TrayBackend`]; the Windows backend
//! (the `tray-icon` crate) is in the `shell` module.

use std::sync::Arc;

use tokenmon_core::ProviderId;
use tokenmon_core::settings::Settings;
use tokenmon_ui::tray_icon::{
    MenuAction, MenuEntry, MenuInputs, TaskbarTheme, TrayBitmap, TrayContent, TrayIconCache,
    TrayIconRequest, TrayIconSize, TrayInputs, build_menu, tooltip_for_content,
};

/// What the controller drives: the OS tray icon.
pub trait TrayBackend {
    /// Replaces the icon bitmap and the tooltip together. Returns whether the
    /// OS accepted both; on `false` the controller tries again on its next
    /// refresh instead of believing the icon is current.
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str) -> bool;
}

/// Live data the tray reflects.
pub struct TrayData<'a> {
    /// Current settings.
    pub settings: &'a Settings,
    /// Headline percentage per provider that has one.
    pub used_percent: &'a [(ProviderId, f64)],
    /// Whether the provider has a usable session.
    pub signed_in: &'a dyn Fn(ProviderId) -> bool,
}

/// Renders and caches tray icons and pushes changes to a backend.
pub struct TrayController<B: TrayBackend> {
    backend: B,
    cache: TrayIconCache,
    last_key: Option<String>,
    last_tooltip: Option<String>,
}

impl<B: TrayBackend> TrayController<B> {
    /// A controller over `backend`; the first [`refresh`](Self::refresh)
    /// always updates.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            cache: TrayIconCache::new(),
            last_key: None,
            last_tooltip: None,
        }
    }

    /// The backend (tests read what it received).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// The backend, mutably (the shell swaps the context menu on it).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Recomputes the icon and tooltip; updates the backend only when the
    /// render key or the tooltip changed. Returns whether the backend was
    /// updated successfully; a failed update is retried on the next call.
    ///
    /// Call it on every poll result, settings change, theme change
    /// (`WM_SETTINGCHANGE`) and DPI change (a new `size`).
    pub fn refresh(
        &mut self,
        data: &TrayData<'_>,
        size: TrayIconSize,
        theme: TaskbarTheme,
    ) -> bool {
        let settings = data.settings;
        let order = settings.ordered_usage_providers();
        let enabled: Vec<_> = ProviderId::USAGE_PROVIDERS
            .into_iter()
            .filter(|p| settings.is_provider_enabled(*p))
            .collect();
        let graphs: Vec<_> = settings.menu_bar_graph_providers.iter().copied().collect();
        let content = TrayContent::from_inputs(&TrayInputs {
            selected: settings.selected_provider,
            show_selected: settings.show_selected_provider_in_menu_bar,
            used_percent: data.used_percent,
            provider_order: &order,
            menu_bar_graphs: &graphs,
            enabled: &enabled,
            grok_signed_in: (data.signed_in)(ProviderId::Grok),
        });
        // One entry per composite segment, as the icon draws them (#61): a
        // percent shown in the icon is never replaced by a bare name.
        let tooltip = tooltip_for_content(&content, (data.signed_in)(ProviderId::Grok));
        // The icon is always the TokenMon logo (the macOS menu bar draws
        // provider glyphs and percentages; Windows keeps usage in the tooltip
        // so the mark stays recognisable at 16 px).
        let request = TrayIconRequest {
            content: TrayContent::Mark,
            size,
            theme,
        };
        let key = request.key();
        if self.last_key.as_deref() == Some(&key) && self.last_tooltip.as_deref() == Some(&tooltip)
        {
            return false;
        }
        let bitmap: Arc<TrayBitmap> = self.cache.get_or_render(&request);
        if !self.backend.update(&bitmap, &tooltip) {
            return false;
        }
        self.last_key = Some(key);
        self.last_tooltip = Some(tooltip);
        true
    }
}

/// The right-click menu for `settings`.
pub fn context_menu(settings: &Settings) -> Vec<MenuEntry> {
    let order = settings.ordered_usage_providers();
    let enabled: Vec<_> = ProviderId::USAGE_PROVIDERS
        .into_iter()
        .filter(|p| settings.is_provider_enabled(*p))
        .collect();
    build_menu(&MenuInputs {
        provider_order: &order,
        enabled: &enabled,
    })
}

/// Which provider the panel should show for a menu action (`None` for
/// actions that do not open the panel).
pub fn panel_target(action: MenuAction) -> Option<ProviderId> {
    match action {
        MenuAction::OpenProvider(p) => Some(p),
        MenuAction::OpenOverview => Some(ProviderId::Overview),
        MenuAction::Settings | MenuAction::CheckForUpdates | MenuAction::Quit => None,
    }
}

/// What choosing a context-menu item asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    /// Open the panel on this tab (Overview included).
    Show(ProviderId),
    /// Open the Settings window.
    OpenSettings,
    /// Run an update check now.
    CheckForUpdates,
    /// Quit TokenMon.
    Quit,
}

/// Maps a menu item to its command.
pub fn menu_command(action: MenuAction) -> MenuCommand {
    match action {
        MenuAction::OpenProvider(p) => MenuCommand::Show(p),
        MenuAction::OpenOverview => MenuCommand::Show(ProviderId::Overview),
        MenuAction::Settings => MenuCommand::OpenSettings,
        MenuAction::CheckForUpdates => MenuCommand::CheckForUpdates,
        MenuAction::Quit => MenuCommand::Quit,
    }
}

/// The pixel edge to render the tray icon at: the system's small-icon metric
/// for the monitor's DPI (`SM_CXSMICON`), or 16 px scaled by DPI when the
/// metric is unavailable.
///
/// ```
/// use tokenmon_app::tray::icon_size;
/// use tokenmon_ui::tray_icon::TrayIconSize;
/// assert_eq!(icon_size(16, 96), TrayIconSize::Px16);
/// assert_eq!(icon_size(0, 144), TrayIconSize::Px24);
/// assert_eq!(icon_size(40, 240), TrayIconSize::Px40);
/// ```
pub fn icon_size(small_icon_metric: u32, dpi: u32) -> TrayIconSize {
    let px = if small_icon_metric > 0 {
        small_icon_metric
    } else {
        (16 * dpi.max(96)).div_ceil(96)
    };
    TrayIconSize::nearest(px)
}

/// A screen rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: i32,
    /// Height.
    pub height: i32,
}

/// What a completed tray click does to the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    /// Open the panel.
    Show,
    /// The panel is open but covered by other windows: raise it.
    BringToFront,
    /// The panel is open and in front: close it.
    Hide,
}

/// The tray-click toggle. The panel does not dismiss itself when another
/// window is clicked (maintainer decision on #81), so it can sit behind other
/// windows; clicking the icon then raises it, and only a panel that is already
/// the foreground window is closed by the click.
///
/// ```
/// use tokenmon_app::tray::{ClickAction, click_action};
/// assert_eq!(click_action(false, false), ClickAction::Show);
/// assert_eq!(click_action(true, false), ClickAction::BringToFront);
/// assert_eq!(click_action(true, true), ClickAction::Hide);
/// ```
pub fn click_action(visible: bool, was_foreground: bool) -> ClickAction {
    match (visible, was_foreground) {
        (false, _) => ClickAction::Show,
        (true, false) => ClickAction::BringToFront,
        (true, true) => ClickAction::Hide,
    }
}

/// How long before a tray press the panel can have lost focus and still count
/// as having been the foreground window: pressing the tray icon moves focus
/// to the taskbar first, so by the time the press is reported the panel is
/// already inactive.
pub const PRESS_FOCUS_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether the panel was the foreground window when a tray press happened:
/// it has focus now, or lost it within [`PRESS_FOCUS_WINDOW`] (to the
/// taskbar, because of this very press).
///
/// ```
/// use std::time::Duration;
/// use tokenmon_app::tray::was_foreground;
/// assert!(was_foreground(true, None));
/// assert!(was_foreground(false, Some(Duration::from_millis(20))));
/// assert!(!was_foreground(false, Some(Duration::from_secs(5))));
/// assert!(!was_foreground(false, None));
/// ```
pub fn was_foreground(focused: bool, since_lost: Option<std::time::Duration>) -> bool {
    focused || since_lost.is_some_and(|t| t < PRESS_FOCUS_WINDOW)
}
