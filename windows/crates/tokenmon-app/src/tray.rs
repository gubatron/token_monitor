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
    MenuAction, MenuEntry, MenuInputs, TaskbarTheme, TooltipEntry, TrayBitmap, TrayContent,
    TrayIconCache, TrayIconRequest, TrayIconSize, TrayInputs, build_menu, build_tooltip,
};

/// What the controller drives: the OS tray icon.
pub trait TrayBackend {
    /// Replaces the icon bitmap and the tooltip together.
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str);
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
    /// render key or the tooltip changed. Returns whether it did.
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
        let request = TrayIconRequest {
            content,
            size,
            theme,
        };
        let tooltip = build_tooltip(
            &order
                .iter()
                .filter(|p| enabled.contains(p))
                .map(|p| TooltipEntry {
                    provider: *p,
                    used: data
                        .used_percent
                        .iter()
                        .find(|(q, _)| q == p)
                        .map(|(_, v)| *v),
                    signed_out: !(data.signed_in)(*p),
                })
                .collect::<Vec<_>>(),
        );
        let key = request.key();
        if self.last_key.as_deref() == Some(&key) && self.last_tooltip.as_deref() == Some(&tooltip)
        {
            return false;
        }
        let bitmap: Arc<TrayBitmap> = self.cache.get_or_render(&request);
        self.backend.update(&bitmap, &tooltip);
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

/// Gap between the tray icon and the panel, in pixels.
pub const PANEL_GAP: i32 = 8;

/// Where to put a `size` panel opened from the tray icon at `anchor` (the
/// icon's top centre): above the icon when there is room, else below it, then
/// clamped entirely inside `work` (the monitor's work area, so the taskbar is
/// never covered). A panel larger than the work area pins to its top left.
pub fn place_panel(anchor: (i32, i32), size: (i32, i32), work: Rect) -> (i32, i32) {
    let (w, h) = size;
    let above = anchor.1 - h - PANEL_GAP;
    let y = if above >= work.y {
        above
    } else {
        anchor.1 + PANEL_GAP
    };
    let x = anchor.0 - w / 2;
    let max_x = (work.x + work.width - w).max(work.x);
    let max_y = (work.y + work.height - h).max(work.y);
    (x.clamp(work.x, max_x), y.clamp(work.y, max_y))
}
