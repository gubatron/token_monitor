//! View-model behavior of the Settings window (spec 01 section 2.1).
#![allow(missing_docs)]

mod common;

use common::fixture;
use tokenmon_core::ProviderId;
use tokenmon_ui::settings::fake::HostCall;
use tokenmon_ui::settings::model::{ACTIVE_POLL_RANGE, IDLE_POLL_RANGE};
use tokenmon_ui::settings::screen::{
    ACCOUNT_PROVIDERS, ACCOUNTS_PRIVACY_NOTE, GROK_CATEGORIES_ID, HISTORY_STORE_FAILED,
    MENU_BAR_FOOTER_OFF, MENU_BAR_FOOTER_ON, SettingsTab, account_subtitle, any_signed_in,
    tab_on_open, threshold_text,
};
use tokenmon_ui::settings::view_model::{Step, stepped};
use tokenmon_ui::settings::{
    AccountStatus, ExportFormat, HostStatus, ProductCategory, RefreshStatus, SettingsBackend,
};

/// Steppers move by their step and clamp at both ends.
#[test]
fn test_stepper_clamps_to_range() {
    assert_eq!(stepped(60, Step::Up, ACTIVE_POLL_RANGE), 75);
    assert_eq!(stepped(15, Step::Down, ACTIVE_POLL_RANGE), 15);
    assert_eq!(stepped(300, Step::Up, ACTIVE_POLL_RANGE), 300);
    assert_eq!(stepped(60, Step::Down, IDLE_POLL_RANGE), 60);
    assert_eq!(stepped(3600, Step::Up, IDLE_POLL_RANGE), 3600);
    assert_eq!(stepped(300, Step::Up, IDLE_POLL_RANGE), 360);
}

/// General tab defaults: strings, flags, stepper states.
#[test]
fn test_general_defaults() {
    let f = fixture(None);
    let g = f.vm.screen().general;
    assert_eq!(g.active_text, "60s");
    assert_eq!(g.idle_text, "300s");
    assert_eq!(g.version_text, "Version unknown");
    assert!(g.checks_for_updates);
    assert!(!g.threshold_enabled);
    assert_eq!(g.threshold_text, "Alert at 80%");
    assert_eq!(g.refresh_lines, Vec::<String>::new());
    assert_eq!(g.alert_rows.len(), 7);
    assert!(!g.desktop_menu, "desktop menu is off by default");
}

/// Stepper intents write through and reflect the clamp at the bounds.
#[test]
fn test_step_active_and_idle_write_through() {
    let f = fixture(None);
    for _ in 0..5 {
        f.vm.step_active(Step::Down);
    }
    let g = f.vm.screen().general;
    assert_eq!(g.active_text, "15s");
    assert!(!g.active_can_decrement);
    assert!(g.active_can_increment);
    f.vm.step_idle(Step::Up);
    assert_eq!(f.vm.screen().general.idle_text, "360s");
}

/// Slider values are rounded and clamped to 50-99.
#[test]
fn test_threshold_clamped_and_rounded() {
    let f = fixture(None);
    f.vm.set_threshold_percent(12.0);
    assert_eq!(f.settings.snapshot().threshold_percent, 50.0);
    f.vm.set_threshold_percent(150.0);
    assert_eq!(f.settings.snapshot().threshold_percent, 99.0);
    f.vm.set_threshold_percent(72.6);
    assert_eq!(f.vm.screen().general.threshold_text, "Alert at 73%");
    assert_eq!(threshold_text(80.0), "Alert at 80%");
}

/// Alert rows list only visible (enabled) providers, in the user's order.
#[test]
fn test_alert_rows_follow_visible_providers_and_order() {
    let f = fixture(None);
    f.vm.set_provider_enabled("cursor", false);
    f.vm.move_provider("claude", 0);
    let ids: Vec<_> =
        f.vm.screen()
            .general
            .alert_rows
            .into_iter()
            .map(|r| r.id)
            .collect();
    assert_eq!(
        ids,
        [
            "claude",
            "grok",
            "opencode",
            "chatgpt",
            "openrouter",
            "grokbot"
        ]
    );
    f.vm.set_alert_provider("claude", false);
    assert!(!f.vm.screen().general.alert_rows[0].on);
}

/// Refresh section: per-provider lines for Overview, last refresh plus error otherwise.
#[test]
fn test_refresh_status_rendering() {
    let f = fixture(None);
    *f.host.status.lock().unwrap() = HostStatus {
        refresh: RefreshStatus::PerProvider(vec![(ProviderId::Grok, "Nov 6, 10:01".into())]),
        ..Default::default()
    };
    assert_eq!(f.vm.screen().general.refresh_lines, ["Grok: Nov 6, 10:01"]);
    f.host.status.lock().unwrap().refresh = RefreshStatus::Single {
        last_refresh: "10:02".into(),
        last_error: Some("HTTP 500".into()),
    };
    let g = f.vm.screen().general;
    assert_eq!(g.refresh_lines, ["Last refresh: 10:02"]);
    assert_eq!(g.refresh_error, "HTTP 500");
}

/// Launch-at-login writes through the host and the next screen shows the real state.
#[test]
fn test_launch_at_login_and_desktop_menu_use_host() {
    let f = fixture(None);
    f.vm.set_launch_at_login(true);
    f.vm.set_desktop_menu(true);
    let g = f.vm.screen().general;
    assert!(g.launch_at_login && g.desktop_menu);
    assert_eq!(
        f.host.calls(),
        [HostCall::LaunchAtLogin(true), HostCall::DesktopMenu(true)]
    );
}

/// A failed autostart write leaves the real state (off) and shows the error.
#[test]
fn test_failed_launch_at_login_reverts_and_reports() {
    let f = fixture(None);
    *f.host.fail_with.lock().unwrap() = Some("registry denied".into());
    f.vm.set_launch_at_login(true);
    let g = f.vm.screen().general;
    assert!(!g.launch_at_login);
    assert_eq!(g.refresh_error, "registry denied");
}

/// Version row and update button come from the host.
#[test]
fn test_update_row_from_host_status() {
    let f = fixture(None);
    {
        let mut st = f.host.status.lock().unwrap();
        st.app_version = Some("1.8.0".into());
        st.update_action_title = "Check Now".into();
        st.update_can_act = true;
        st.update_status_message = Some("Up to date".into());
    }
    let g = f.vm.screen().general;
    assert_eq!(g.version_text, "Version 1.8.0");
    assert_eq!(g.update_action_title, "Check Now");
    assert!(g.update_can_act);
    assert_eq!(g.update_status_message, "Up to date");
    f.vm.update_primary_action();
    assert_eq!(f.host.calls(), [HostCall::UpdateAction]);
}

/// The last enabled provider cannot be switched off, and its toggle is disabled.
#[test]
fn test_last_enabled_provider_cannot_be_disabled() {
    let f = fixture(None);
    for id in [
        "cursor",
        "opencode",
        "claude",
        "chatgpt",
        "openrouter",
        "grokbot",
    ] {
        f.vm.set_provider_enabled(id, false);
    }
    f.vm.set_provider_enabled("grok", false);
    let rows = f.vm.screen().providers.rows;
    let grok = rows.iter().find(|r| r.id == "grok").unwrap();
    assert!(grok.enabled);
    assert!(!grok.toggle_enabled);
    assert!(
        rows.iter()
            .filter(|r| r.id != "grok")
            .all(|r| r.toggle_enabled)
    );
}

/// Dragging claude onto grok gives [claude, grok, ...] and rows keep identity.
#[test]
fn test_move_provider_matches_spec_example() {
    let f = fixture(None);
    f.vm.move_provider("claude", 0);
    let rows = f.vm.screen().providers.rows;
    let pos = |id: &str| rows.iter().find(|r| r.id == id).unwrap().position;
    assert_eq!(pos("claude"), 0);
    assert_eq!(pos("grok"), 1);
    assert_eq!(pos("cursor"), 2);
    assert_eq!(pos("opencode"), 3);
    // Row identity stays in default order so the drag animates.
    assert_eq!(rows[0].id, "grok");
}

/// Out-of-range or unknown moves are ignored.
#[test]
fn test_move_provider_ignores_bad_input() {
    let f = fixture(None);
    let before = f.settings.snapshot();
    f.vm.move_provider("claude", 99);
    f.vm.move_provider("claude", -1);
    f.vm.move_provider("nope", 0);
    assert_eq!(f.settings.snapshot(), before);
}

/// Menu Bar: graph rows hidden when "selected only" is on; Grok categories
/// follows the Grok row; footers switch.
#[test]
fn test_menu_bar_section_rows_and_footers() {
    let f = fixture(None);
    let p = f.vm.screen().providers;
    let labels: Vec<_> = p.graph_rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Grok bar graph",
            "Grok categories",
            "Grokbot bar graph",
            "Claude bar graph",
            "Cursor bar graph",
            "OpenCode bar graph"
        ]
    );
    assert_eq!(p.menu_bar_footer, MENU_BAR_FOOTER_OFF);
    f.vm.set_show_selected_only(true);
    let p = f.vm.screen().providers;
    assert_eq!(p.graph_rows, []);
    assert_eq!(p.menu_bar_footer, MENU_BAR_FOOTER_ON);
}

/// Graph switches write the right setting; ChatGPT cannot be pinned.
#[test]
fn test_menu_bar_graph_toggles() {
    let f = fixture(None);
    f.vm.set_menu_bar_graph("claude", true);
    f.vm.set_menu_bar_graph("chatgpt", true);
    f.vm.set_menu_bar_graph(GROK_CATEGORIES_ID, false);
    let s = f.settings.snapshot();
    assert!(s.menu_bar_graph_providers.contains(&ProviderId::Claude));
    assert!(!s.menu_bar_graph_providers.contains(&ProviderId::ChatGpt));
    assert!(!s.show_categories_in_menu_bar);
}

/// Account subtitles: email, "Signed in", "Not signed in".
#[test]
fn test_account_subtitles() {
    let mk = |signed_in, email: Option<&str>| AccountStatus {
        provider: ProviderId::Grok,
        signed_in,
        email: email.map(str::to_owned),
        last_error: None,
    };
    assert_eq!(account_subtitle(&mk(true, Some("a@b.com"))), "a@b.com");
    assert_eq!(account_subtitle(&mk(true, None)), "Signed in");
    assert_eq!(account_subtitle(&mk(true, Some(""))), "Signed in");
    assert_eq!(
        account_subtitle(&mk(false, Some("a@b.com"))),
        "Not signed in"
    );
}

/// Accounts rows: fixed order, missing statuses read "Not signed in", errors surface.
#[test]
fn test_accounts_rows_order_and_errors() {
    let f = fixture(None);
    f.host.status.lock().unwrap().accounts = vec![AccountStatus {
        provider: ProviderId::Claude,
        signed_in: false,
        email: None,
        last_error: Some("session expired".into()),
    }];
    let a = f.vm.screen().accounts;
    let ids: Vec<_> = a.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["grok", "claude", "chatgpt", "cursor", "opencode"]);
    assert_eq!(ACCOUNT_PROVIDERS.len(), a.rows.len());
    assert_eq!(a.rows[1].error, "session expired");
    assert_eq!(a.rows[0].subtitle, "Not signed in");
}

/// Sign in / re-authenticate / sign out reach the host; OpenRouter has no window.
#[test]
fn test_sign_in_out_callbacks() {
    let f = fixture(None);
    f.vm.sign_in("grok");
    f.vm.sign_in("openrouter");
    f.vm.sign_in("overview");
    f.vm.sign_out("cursor");
    assert_eq!(
        f.host.calls(),
        [
            HostCall::SignIn(ProviderId::Grok),
            HostCall::SignOut(ProviderId::Cursor)
        ]
    );
}

/// OpenRouter save trims, ignores empty drafts, reports failures, clears on success.
#[test]
fn test_openrouter_save_flow() {
    let f = fixture(None);
    assert!(!f.vm.save_openrouter_key("   "));
    assert_eq!(f.host.calls(), []);
    *f.host.fail_with.lock().unwrap() = Some("Enter an OpenRouter API key.".into());
    assert!(!f.vm.save_openrouter_key(" sk-bad "));
    assert_eq!(
        f.vm.screen().accounts.openrouter_error,
        "Enter an OpenRouter API key."
    );
    *f.host.fail_with.lock().unwrap() = None;
    assert!(f.vm.save_openrouter_key(" sk-or-v1-abc "));
    assert_eq!(f.vm.screen().accounts.openrouter_error, "");
    assert_eq!(
        f.host.calls().last(),
        Some(&HostCall::SaveOpenRouterKey("sk-or-v1-abc".into()))
    );
    f.vm.sign_out_openrouter();
    assert_eq!(f.host.calls().last(), Some(&HostCall::SignOutOpenRouter));
}

/// Data tab: one row per category, labelled with display names, toggles persist.
#[test]
fn test_category_rows_and_toggle() {
    let f = fixture(None);
    let labels: Vec<_> =
        f.vm.screen()
            .data
            .category_rows
            .iter()
            .map(|r| r.label.clone())
            .collect();
    assert_eq!(
        labels,
        ["Grok Build", "API", "Chat", "Imagine", "Voice", "Other"]
    );
    f.vm.set_product_visible("voice", false);
    f.vm.set_product_visible("bogus", false);
    let s = f.settings.snapshot();
    assert!(!s.visible_products.contains(&ProductCategory::Voice));
    assert_eq!(s.visible_products.len(), 5);
}

/// Export opens the dialog with the default name and writes via the host.
#[test]
fn test_export_uses_default_name_and_chosen_path() {
    let f = fixture(Some("C:/out/h.csv"));
    f.vm.export_history(ExportFormat::Csv);
    f.vm.export_history(ExportFormat::Json);
    assert_eq!(
        *f.dialog.requested.lock().unwrap(),
        ["grok-usage.csv", "grok-usage.json"]
    );
    assert_eq!(
        f.host.calls()[0],
        HostCall::Export(ExportFormat::Csv, "C:/out/h.csv".into())
    );
}

/// A cancelled dialog exports nothing and a new export clears the old error.
#[test]
fn test_export_cancel_and_error_clearing() {
    let f = fixture(Some("x.json"));
    *f.host.fail_with.lock().unwrap() = Some("disk full".into());
    f.vm.export_history(ExportFormat::Json);
    assert_eq!(f.vm.screen().data.export_error, "disk full");
    *f.host.fail_with.lock().unwrap() = None;
    f.vm.export_history(ExportFormat::Json);
    assert_eq!(f.vm.screen().data.export_error, "");

    let cancelled = fixture(None);
    cancelled.vm.export_history(ExportFormat::Csv);
    assert_eq!(cancelled.host.calls(), []);
}

/// Clear history and the red storage-failed notice.
#[test]
fn test_clear_history_and_store_failed_notice() {
    let f = fixture(None);
    assert_eq!(f.vm.screen().data.store_failed_text, "");
    f.host.status.lock().unwrap().history_store_failed = true;
    assert_eq!(f.vm.screen().data.store_failed_text, HISTORY_STORE_FAILED);
    f.vm.clear_history();
    f.vm.quit();
    assert_eq!(f.host.calls(), [HostCall::ClearHistory, HostCall::Quit]);
}

/// Export format names roundtrip and reject unknowns.
#[test]
fn test_export_format_names() {
    for f in [ExportFormat::Csv, ExportFormat::Json] {
        assert_eq!(ExportFormat::from_extension(f.extension()), Some(f));
        assert!(f.default_file_name().ends_with(f.extension()));
    }
    assert_eq!(ExportFormat::from_extension("xml"), None);
    assert_eq!(ProductCategory::from_id("API"), Some(ProductCategory::Api));
    assert_eq!(ProductCategory::from_id("x"), None);
}

fn signed_in(p: ProviderId) -> AccountStatus {
    AccountStatus {
        provider: p,
        signed_in: true,
        email: None,
        last_error: None,
    }
}

/// Nothing signed in forces Accounts whatever tab was last; any session or an
/// OpenRouter key keeps the last tab.
#[test]
fn test_tab_on_open_first_run_selects_accounts() {
    for last in [
        SettingsTab::General,
        SettingsTab::Providers,
        SettingsTab::Accounts,
        SettingsTab::Data,
    ] {
        assert_eq!(tab_on_open(false, last), SettingsTab::Accounts);
        assert_eq!(tab_on_open(true, last), last);
    }
}

/// Signed-in detection covers every cookie provider and OpenRouter.
#[test]
fn test_any_signed_in_covers_sessions_and_openrouter_key() {
    assert!(!any_signed_in(&HostStatus::default()));
    for p in ACCOUNT_PROVIDERS {
        let host = HostStatus {
            accounts: vec![signed_in(p)],
            ..HostStatus::default()
        };
        assert!(any_signed_in(&host), "{p:?}");
    }
    let mut host = HostStatus::default();
    host.openrouter.connected = true;
    assert!(any_signed_in(&host));
}

/// The view model reads the host status when deciding the opening tab.
#[test]
fn test_view_model_opening_tab_follows_host_status() {
    let f = fixture(None);
    assert_eq!(
        f.vm.opening_tab(SettingsTab::General),
        SettingsTab::Accounts
    );
    f.host.status.lock().unwrap().accounts = vec![signed_in(ProviderId::Claude)];
    assert_eq!(f.vm.opening_tab(SettingsTab::Data), SettingsTab::Data);
}

/// Tab indices round-trip and unknown indices fall back to General.
#[test]
fn test_settings_tab_index_round_trip() {
    for t in [
        SettingsTab::General,
        SettingsTab::Providers,
        SettingsTab::Accounts,
        SettingsTab::Data,
    ] {
        assert_eq!(SettingsTab::from_index(t.index()), t);
    }
    assert_eq!(SettingsTab::from_index(9), SettingsTab::General);
}

/// The Accounts screen carries the privacy note, and it stays honest about
/// the plaintext fallback ("normally encrypted") and the storage path.
#[test]
fn test_accounts_screen_has_truthful_privacy_note() {
    let f = fixture(None);
    let note = f.vm.screen().accounts.privacy_note;
    assert_eq!(note, ACCOUNTS_PRIVACY_NOTE);
    assert!(note.contains("no servers"));
    assert!(note.contains("normally encrypted"));
    assert!(note.contains("Windows Data Protection"));
    assert!(note.contains(r"%LOCALAPPDATA%\TokenMon"));
    assert!(note.contains("signing out deletes it"));
}
