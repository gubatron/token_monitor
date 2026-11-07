//! Tray icon wiring: the real tray icon, its click and menu events, and the
//! periodic UI tick. Windows glue only; every decision is in `crate::tray`,
//! `crate::placement` and `crate::panel`.

use super::{
    Arc, Calendar, ComponentHandle, DateTime, Duration, Icon, LOCAL_CLOCK, Menu, MenuAction,
    MenuEntry, MenuEvent, MenuItem, MouseButton, MouseButtonState, PredefinedMenuItem, ProviderId,
    Rc, Rect, StartupFailure, TICK, TRAY_ATTEMPTS, TrayBackend, TrayBitmap, TrayController,
    TrayData, TrayIcon, TrayIconBuilder, TrayIconEvent, Ui, Utc, center, click_action,
    context_menu, debug_panel, default_anchor, display, icon_size, menu_command, next_midnight,
    now_and_clock, record_live, refresh, resolve_anchor, retry, system, theme, with_ui,
};

/// Drives the real tray icon.
struct TrayIconBackend(TrayIcon);

impl TrayBackend for TrayIconBackend {
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str) -> bool {
        let icon = match Icon::from_rgba(bitmap.rgba.clone(), bitmap.size, bitmap.size) {
            Ok(icon) => icon,
            Err(e) => {
                tracing::warn!(error = %e, "tray icon bitmap rejected");
                return false;
            }
        };
        // Both are attempted; a failure of either is retried on the next tick.
        let icon_set = self.0.set_icon(Some(icon));
        let tip_set = self.0.set_tooltip(Some(tooltip));
        for e in [icon_set.as_ref().err(), tip_set.as_ref().err()]
            .into_iter()
            .flatten()
        {
            tracing::warn!(error = %e, "tray icon update failed; will retry");
        }
        icon_set.is_ok() && tip_set.is_ok()
    }
}

fn tray_menu(entries: &[MenuEntry]) -> Menu {
    let menu = Menu::new();
    for entry in entries {
        match entry {
            MenuEntry::Item { label, action } => {
                let _ = menu.append(&MenuItem::with_id(
                    action.id().to_string(),
                    label,
                    true,
                    None,
                ));
            }
            MenuEntry::Separator => {
                let _ = menu.append(&PredefinedMenuItem::separator());
            }
        }
    }
    menu
}

/// What the periodic UI tick remembers between runs.
struct TickState {
    controller: TrayController<TrayIconBackend>,
    menu: Vec<MenuEntry>,
    next_midnight: DateTime<Utc>,
}

/// Creates the tray icon and routes its clicks and menu.
///
/// # Errors
/// [`StartupFailure::Tray`] when the icon cannot be created after retries.
pub(super) fn wire_tray(ui: &Rc<Ui>, show_panel: bool) -> Result<(), StartupFailure> {
    let settings = ui.app.settings.snapshot();
    let entries = context_menu(&settings);
    let build = || -> Result<TrayIcon, String> {
        let icon = Icon::from_rgba(vec![0; 16 * 16 * 4], 16, 16).map_err(|e| e.to_string())?;
        TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("TokenMon")
            .with_menu(Box::new(tray_menu(&entries)))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())
    };
    let tray = retry(
        TRAY_ATTEMPTS,
        |failures| {
            tracing::warn!(failures, "tray icon not created yet; retrying");
            std::thread::sleep(Duration::from_secs(1));
        },
        build,
    )
    .map_err(StartupFailure::Tray)?;

    // Any tray event tells where the icon is; a left click toggles the panel.
    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        #[derive(Clone, Copy)]
        enum Phase {
            Hover,
            Press,
            Release,
        }
        let (rect, phase) = match &event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Down,
                rect,
                ..
            } => (*rect, Phase::Press),
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } => (*rect, Phase::Release),
            TrayIconEvent::Click { rect, .. }
            | TrayIconEvent::Enter { rect, .. }
            | TrayIconEvent::Move { rect, .. }
            | TrayIconEvent::Leave { rect, .. } => (*rect, Phase::Hover),
            _ => return,
        };
        // Resolved now, while the overflow flyout (if the icon lives there)
        // is still open: an icon inside it anchors to the notification area.
        let icon = Rect {
            x: rect.position.x as i32,
            y: rect.position.y as i32,
            width: i32::try_from(rect.size.width).unwrap_or(0),
            height: i32::try_from(rect.size.height).unwrap_or(0),
        };
        let probe = display::monitor_at(center(icon).0, center(icon).1);
        let overflow = system::overflow_flyout_rect();
        let notify = system::notification_area_rect();
        let (anchor, source) = resolve_anchor(
            Some(icon),
            overflow,
            notify,
            probe.monitor,
            probe.work,
            probe.dpi,
        );
        if debug_panel() && !matches!(phase, Phase::Hover) {
            tracing::info!(
                ?icon,
                ?overflow,
                ?notify,
                ?anchor,
                ?source,
                dpi = probe.dpi,
                "panel: tray event anchor"
            );
        }
        with_ui(move |ui| {
            ui.anchor.set(Some(anchor));
            match phase {
                Phase::Hover => {}
                // The press already moved focus to the taskbar: decide now,
                // from whether the panel was in front just before it.
                Phase::Press => {
                    let action = click_action(ui.is_panel_visible(), ui.panel_was_foreground());
                    if debug_panel() {
                        tracing::info!(?action, "panel: tray press");
                    }
                    ui.pending_click.set(Some(action));
                }
                Phase::Release => ui.finish_tray_click(),
            }
        });
    }));

    // Right-click menu actions.
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let Some(action) = event.id.0.parse().ok().and_then(MenuAction::from_id) else {
            return;
        };
        with_ui(move |ui| ui.run_command(menu_command(action)));
    }));

    let mut tick = TickState {
        controller: TrayController::new(TrayIconBackend(tray)),
        menu: entries,
        next_midnight: next_midnight(Utc::now(), &Calendar::local(&LOCAL_CLOCK)),
    };
    let weak = Rc::downgrade(ui);
    ui.tick.start(slint::TimerMode::Repeated, TICK, move || {
        if let Some(ui) = weak.upgrade() {
            on_tick(&ui, &mut tick);
        }
    });

    if show_panel {
        ui.show_selected();
    }
    Ok(())
}

/// The 2 s tick: re-renders the tray (usage, settings, taskbar theme, DPI and
/// icon-size changes all show up as a different render key; the controller
/// touches the OS only when it changed), swaps the context menu when the
/// provider set changed, keeps an open panel live, rolls the Overview over at
/// local midnight and records the Overview history.
fn on_tick(ui: &Rc<Ui>, tick: &mut TickState) {
    let (now, clock) = now_and_clock();
    let settings = ui.app.settings.snapshot();
    let used: Vec<_> = ui
        .app
        .model
        .registry()
        .entries()
        .iter()
        .filter_map(|e| e.poller.headline_used_percent().map(|v| (e.provider, v)))
        .collect();
    let sessions = Arc::clone(&ui.app.sessions);
    let signed_in = move |p: ProviderId| sessions.signed_in(p);
    let anchor = ui.anchor.get().unwrap_or_else(default_anchor);
    let dpi = display::monitor_at(center(anchor).0, center(anchor).1).dpi;
    tick.controller.refresh(
        &TrayData {
            settings: &settings,
            used_percent: &used,
            signed_in: &signed_in,
        },
        icon_size(display::small_icon_metric(dpi), dpi),
        theme::taskbar_theme(),
    );

    let entries = context_menu(&settings);
    if entries != tick.menu {
        tick.controller
            .backend_mut()
            .0
            .set_menu(Some(Box::new(tray_menu(&entries))));
        tick.menu = entries;
    }

    if now >= tick.next_midnight {
        tick.next_midnight = next_midnight(now, &Calendar::local(clock));
        ui.state.borrow_mut().reset_offsets();
    }
    // Cheap and written only when the series changed, so a poll just before
    // midnight reaches yesterday's row within one tick.
    record_live(
        &ui.store,
        ui.app.model.registry(),
        &settings,
        &ui.app.sessions,
        now,
        clock,
    );
    // A panel hidden by some other route (Alt+F4) must not keep the pollers
    // on the active cadence.
    ui.app.model.sync_menu_open(ui.is_panel_visible());
    if ui.is_panel_visible() {
        ui.render(false);
    }
    if let Some(window) = ui.settings_window.borrow().as_ref()
        && window.window().is_visible()
    {
        refresh(window, &ui.settings_vm);
    }
}
