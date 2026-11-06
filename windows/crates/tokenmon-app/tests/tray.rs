//! Tray controller: NIM_MODIFY only when the rendered key or tooltip changes
//! (spec 04 section 2) and the right-click menu.

use tokenmon_app::tray::{TrayBackend, TrayController, TrayData, context_menu, panel_target};
use tokenmon_core::ProviderId;
use tokenmon_core::settings::Settings;
use tokenmon_ui::tray_icon::{MenuAction, MenuEntry, TaskbarTheme, TrayBitmap, TrayIconSize};

#[derive(Default)]
struct Recorder {
    updates: Vec<String>,
    /// Fail this many upcoming updates.
    failures: u32,
}

impl TrayBackend for Recorder {
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str) -> bool {
        assert!(bitmap.size > 0);
        self.updates.push(tooltip.to_owned());
        if self.failures > 0 {
            self.failures -= 1;
            return false;
        }
        true
    }
}

fn refresh(
    c: &mut TrayController<Recorder>,
    s: &Settings,
    used: &[(ProviderId, f64)],
    size: TrayIconSize,
    theme: TaskbarTheme,
) -> bool {
    c.refresh(
        &TrayData {
            settings: s,
            used_percent: used,
            signed_in: &|_| true,
        },
        size,
        theme,
    )
}

/// An unchanged state never touches the OS again.
#[test]
fn test_unchanged_state_does_not_modify_the_icon() {
    let s = Settings::default();
    let mut c = TrayController::new(Recorder::default());
    let used = [(ProviderId::Grok, 40.0)];
    assert!(refresh(
        &mut c,
        &s,
        &used,
        TrayIconSize::Px16,
        TaskbarTheme::Dark
    ));
    assert!(!refresh(
        &mut c,
        &s,
        &used,
        TrayIconSize::Px16,
        TaskbarTheme::Dark
    ));
    assert_eq!(c.backend().updates.len(), 1);
}

/// A usage change, a theme change and a DPI change each re-render once.
#[test]
fn test_usage_theme_and_dpi_changes_rerender() {
    let s = Settings::default();
    let mut c = TrayController::new(Recorder::default());
    let a = [(ProviderId::Grok, 40.0)];
    let b = [(ProviderId::Grok, 80.0)];
    assert!(refresh(
        &mut c,
        &s,
        &a,
        TrayIconSize::Px16,
        TaskbarTheme::Dark
    ));
    assert!(refresh(
        &mut c,
        &s,
        &b,
        TrayIconSize::Px16,
        TaskbarTheme::Dark
    ));
    assert!(refresh(
        &mut c,
        &s,
        &b,
        TrayIconSize::Px16,
        TaskbarTheme::Light
    ));
    assert!(refresh(
        &mut c,
        &s,
        &b,
        TrayIconSize::Px32,
        TaskbarTheme::Light
    ));
    assert_eq!(c.backend().updates.len(), 4);
}

/// The menu ends with Settings, Check for Updates and Quit, and menu actions
/// map to the panel target.
#[test]
fn test_menu_and_panel_targets() {
    let menu = context_menu(&Settings::default());
    let actions: Vec<_> = menu
        .iter()
        .filter_map(|e| match e {
            MenuEntry::Item { action, .. } => Some(*action),
            MenuEntry::Separator => None,
        })
        .collect();
    assert_eq!(actions.last(), Some(&MenuAction::Quit));
    assert!(actions.contains(&MenuAction::Settings));
    assert_eq!(
        panel_target(MenuAction::OpenProvider(ProviderId::Claude)),
        Some(ProviderId::Claude)
    );
    assert_eq!(
        panel_target(MenuAction::OpenOverview),
        Some(ProviderId::Overview)
    );
    assert_eq!(panel_target(MenuAction::Quit), None);
}

/// Every menu item maps to one command, Settings included.
#[test]
fn test_menu_items_map_to_commands() {
    use tokenmon_app::tray::{MenuCommand, menu_command};
    assert_eq!(
        menu_command(MenuAction::OpenProvider(ProviderId::Claude)),
        MenuCommand::Show(ProviderId::Claude)
    );
    assert_eq!(
        menu_command(MenuAction::OpenOverview),
        MenuCommand::Show(ProviderId::Overview)
    );
    assert_eq!(
        menu_command(MenuAction::Settings),
        MenuCommand::OpenSettings
    );
    assert_eq!(
        menu_command(MenuAction::CheckForUpdates),
        MenuCommand::CheckForUpdates
    );
    assert_eq!(menu_command(MenuAction::Quit), MenuCommand::Quit);
}

/// The menu lists Open items only for enabled providers, then Overview,
/// Settings, Check for Updates and Quit.
#[test]
fn test_context_menu_lists_enabled_providers_and_fixed_items() {
    let all = context_menu(&Settings::default());
    let labels = |m: &[MenuEntry]| -> Vec<String> {
        m.iter()
            .filter_map(|e| match e {
                MenuEntry::Item { label, .. } => Some(label.clone()),
                MenuEntry::Separator => None,
            })
            .collect()
    };
    let l = labels(&all);
    assert!(l.contains(&"Open Overview".to_owned()));
    assert!(l.contains(&"Settings\u{2026}".to_owned()));
    assert!(l.contains(&"Check for Updates".to_owned()));
    assert_eq!(l.last().map(String::as_str), Some("Quit TokenMon"));
    assert_eq!(l.iter().filter(|x| x.starts_with("Open ")).count(), 8);
}

/// The icon size follows `SM_CXSMICON` and falls back to DPI scaling.
#[test]
fn test_icon_size_follows_the_system_metric_and_dpi() {
    use tokenmon_app::tray::icon_size;
    assert_eq!(icon_size(16, 96), TrayIconSize::Px16);
    assert_eq!(icon_size(20, 120), TrayIconSize::Px20);
    assert_eq!(icon_size(0, 192), TrayIconSize::Px32);
    assert_eq!(icon_size(0, 0), TrayIconSize::Px16);
}

/// The panel opens above the icon when it fits, below when it does not, and
/// never leaves the work area.
#[test]
fn test_panel_placement_clamps_to_the_work_area() {
    use tokenmon_app::tray::{PANEL_GAP, Rect, place_panel};
    let work = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };
    // Bottom taskbar: anchor at the icon's top, room above; the 8 px gap would
    // reach into the taskbar, so the panel is held to the work area's bottom.
    let (x, y) = place_panel((1000, 1050), (420, 600), work);
    assert_eq!((x, y), (790, 440));
    let (_, y) = place_panel((1000, 800), (420, 600), work);
    assert_eq!(y, 800 - 600 - PANEL_GAP);
    // Icon near the right edge: clamp so the panel stays on screen.
    let (x, _) = place_panel((1915, 1050), (420, 600), work);
    assert_eq!(x, 1920 - 420);
    // Top taskbar: not enough room above, so it opens below.
    let top = Rect {
        x: 0,
        y: 40,
        width: 1920,
        height: 1040,
    };
    let (_, y) = place_panel((900, 40), (420, 600), top);
    assert_eq!(y, 40 + PANEL_GAP);
    // Taller than the work area: pinned to the top left.
    let (x, y) = place_panel((100, 500), (420, 5000), work);
    assert_eq!((x, y), (0, 0));
}

/// A failed icon update is not remembered as done: the next refresh retries
/// even though nothing else changed (#72).
#[test]
fn test_failed_icon_update_is_retried() {
    let s = Settings::default();
    let mut c = TrayController::new(Recorder {
        failures: 2,
        ..Recorder::default()
    });
    let used = [(ProviderId::Grok, 40.0)];
    let go = |c: &mut TrayController<Recorder>| {
        refresh(c, &s, &used, TrayIconSize::Px16, TaskbarTheme::Dark)
    };
    assert!(!go(&mut c));
    assert!(!go(&mut c));
    assert!(go(&mut c));
    assert!(!go(&mut c), "settled once it succeeded");
    assert_eq!(c.backend().updates.len(), 3);
}

/// #61: the tooltip describes the icon's own segments. A selected provider
/// whose icon shows a percent (an OpenCode local estimate while signed out)
/// never degrades to a bare name.
#[test]
fn test_tooltip_follows_the_icon_content_not_the_sign_in_state() {
    let s = Settings {
        selected_provider: ProviderId::OpenCode,
        show_selected_provider_in_menu_bar: true,
        ..Settings::default()
    };
    let mut c = TrayController::new(Recorder::default());
    c.refresh(
        &TrayData {
            settings: &s,
            used_percent: &[(ProviderId::OpenCode, 12.0), (ProviderId::Grok, 40.0)],
            signed_in: &|p| p != ProviderId::OpenCode,
        },
        TrayIconSize::Px16,
        TaskbarTheme::Dark,
    );
    assert_eq!(c.backend().updates, ["OpenCode 12%"]);
}
