//! Tray controller: NIM_MODIFY only when the rendered key or tooltip changes
//! (spec 04 section 2) and the right-click menu.

use tokenmon_app::tray::{TrayBackend, TrayController, TrayData, context_menu, panel_target};
use tokenmon_core::ProviderId;
use tokenmon_core::settings::Settings;
use tokenmon_ui::tray_icon::{MenuAction, MenuEntry, TaskbarTheme, TrayBitmap, TrayIconSize};

#[derive(Default)]
struct Recorder {
    updates: Vec<String>,
}

impl TrayBackend for Recorder {
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str) {
        assert!(bitmap.size > 0);
        self.updates.push(tooltip.to_owned());
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
