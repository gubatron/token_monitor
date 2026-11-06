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
