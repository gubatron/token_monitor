//! Glue between the generated Slint `SettingsWindow` and the
//! [`SettingsViewModel`]: pushes a [`SettingsScreen`] into properties and routes
//! callbacks back into view-model intents.

use std::rc::Rc;
use std::sync::Arc;

use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use super::model::ExportFormat;
use super::screen::{
    ACCOUNTS_FOOTER, ALERTS_FOOTER, AccountRow, CATEGORIES_FOOTER, HISTORY_FOOTER,
    PROVIDERS_FOOTER, ProviderRow, SettingsScreen, SettingsTab, ToggleRow, UPDATES_FOOTER,
};
use super::view_model::{SettingsViewModel, Step};

mod generated {
    #![allow(
        missing_docs,
        clippy::all,
        clippy::pedantic,
        clippy::unwrap_used,
        clippy::todo,
        unused
    )]
    include!(concat!(env!("OUT_DIR"), "/settings.rs"));
}
pub use generated::{AccountRowData, ProviderRowData, SettingsWindow, ToggleRowData};

fn toggle_rows(rows: &[ToggleRow]) -> ModelRc<ToggleRowData> {
    let v: Vec<_> = rows
        .iter()
        .map(|r| ToggleRowData {
            id: r.id.as_str().into(),
            label: r.label.as_str().into(),
            on: r.on,
            enabled: r.enabled,
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

fn color(rgb: u32) -> Color {
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn provider_rows(rows: &[ProviderRow]) -> ModelRc<ProviderRowData> {
    let v: Vec<_> = rows
        .iter()
        .map(|r| ProviderRowData {
            id: r.id.as_str().into(),
            name: r.name.as_str().into(),
            initial: r
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default()
                .into(),
            enabled: r.enabled,
            toggle_enabled: r.toggle_enabled,
            position: r.position,
            accent: color(r.accent_rgb),
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

fn account_rows(rows: &[AccountRow]) -> ModelRc<AccountRowData> {
    let v: Vec<_> = rows
        .iter()
        .map(|r| AccountRowData {
            id: r.id.as_str().into(),
            name: r.name.as_str().into(),
            initial: r
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default()
                .into(),
            subtitle: r.subtitle.as_str().into(),
            signed_in: r.signed_in,
            error: r.error.as_str().into(),
            accent: color(r.accent_rgb),
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

/// Pushes every display value into the window's properties.
#[allow(clippy::many_single_char_names)]
pub fn apply_screen(w: &SettingsWindow, s: &SettingsScreen) {
    let g = &s.general;
    w.set_launch_at_login(g.launch_at_login);
    w.set_desktop_menu(g.desktop_menu);
    w.set_checks_for_updates(g.checks_for_updates);
    w.set_version_text(g.version_text.as_str().into());
    w.set_update_action_title(g.update_action_title.as_str().into());
    w.set_update_can_act(g.update_can_act);
    w.set_update_status_message(g.update_status_message.as_str().into());
    w.set_active_text(g.active_text.as_str().into());
    w.set_active_can_decrement(g.active_can_decrement);
    w.set_active_can_increment(g.active_can_increment);
    w.set_idle_text(g.idle_text.as_str().into());
    w.set_idle_can_decrement(g.idle_can_decrement);
    w.set_idle_can_increment(g.idle_can_increment);
    let lines: Vec<SharedString> = g.refresh_lines.iter().map(|l| l.as_str().into()).collect();
    w.set_refresh_lines(ModelRc::new(VecModel::from(lines)));
    w.set_refresh_error(g.refresh_error.as_str().into());
    w.set_threshold_enabled(g.threshold_enabled);
    w.set_threshold_text(g.threshold_text.as_str().into());
    w.set_threshold_percent(g.threshold_percent as f32);
    w.set_alert_rows(toggle_rows(&g.alert_rows));
    w.set_updates_footer(UPDATES_FOOTER.into());
    w.set_alerts_footer(ALERTS_FOOTER.into());

    let p = &s.providers;
    w.set_provider_rows(provider_rows(&p.rows));
    w.set_providers_footer(PROVIDERS_FOOTER.into());
    w.set_show_selected_only(p.show_selected_only);
    w.set_graph_rows(toggle_rows(&p.graph_rows));
    w.set_menu_bar_footer(p.menu_bar_footer.as_str().into());

    let a = &s.accounts;
    w.set_account_rows(account_rows(&a.rows));
    w.set_accounts_footer(ACCOUNTS_FOOTER.into());
    w.set_accounts_privacy_note(a.privacy_note.as_str().into());
    w.set_openrouter_connected(a.openrouter_connected);
    w.set_openrouter_error(a.openrouter_error.as_str().into());

    let d = &s.data;
    w.set_category_rows(toggle_rows(&d.category_rows));
    w.set_categories_footer(CATEGORIES_FOOTER.into());
    w.set_history_footer(HISTORY_FOOTER.into());
    w.set_export_error(d.export_error.as_str().into());
    w.set_store_failed_text(d.store_failed_text.as_str().into());
}

/// Creates the window, fills it and routes every callback to `vm`.
///
/// Each callback runs the intent and then re-reads the whole screen, so the
/// window always shows the backend's clamped, persisted truth.
#[allow(clippy::needless_pass_by_value)]
pub fn create_settings_window(
    vm: Arc<SettingsViewModel>,
) -> Result<Rc<SettingsWindow>, slint::PlatformError> {
    let w = Rc::new(SettingsWindow::new()?);
    apply_screen(&w, &vm.screen());
    wire(&w, &vm);
    Ok(w)
}

/// Re-reads the view model into `w` (call when the window is shown, and after
/// external status changes such as a completed sign-in).
pub fn refresh(w: &SettingsWindow, vm: &SettingsViewModel) {
    apply_screen(w, &vm.screen());
}

/// Selects the tab Settings opens on (call each time the window is shown, not
/// on every refresh): Accounts on first run, otherwise the tab left open.
pub fn select_opening_tab(w: &SettingsWindow, vm: &SettingsViewModel) {
    let last = SettingsTab::from_index(w.get_current_tab());
    w.set_current_tab(vm.opening_tab(last).index());
}

macro_rules! route {
    ($w:expr, $vm:expr, $setter:ident, |$vmv:ident $(, $arg:ident)*| $body:expr) => {{
        let weak = $w.as_weak();
        let $vmv = Arc::clone($vm);
        $w.$setter(move |$($arg),*| {
            $body;
            if let Some(win) = weak.upgrade() {
                apply_screen(&win, &$vmv.screen());
            }
        });
    }};
}

fn step(d: i32) -> Step {
    if d < 0 { Step::Down } else { Step::Up }
}

fn wire(w: &Rc<SettingsWindow>, vm: &Arc<SettingsViewModel>) {
    route!(w, vm, on_launch_at_login_changed, |vm, v| vm
        .set_launch_at_login(v));
    route!(w, vm, on_desktop_menu_changed, |vm, v| vm
        .set_desktop_menu(v));
    route!(w, vm, on_checks_for_updates_changed, |vm, v| vm
        .set_checks_for_updates(v));
    route!(w, vm, on_update_action, |vm| vm.update_primary_action());
    route!(w, vm, on_active_step, |vm, d| vm.step_active(step(d)));
    route!(w, vm, on_idle_step, |vm, d| vm.step_idle(step(d)));
    route!(w, vm, on_threshold_enabled_changed, |vm, v| vm
        .set_threshold_enabled(v));
    route!(w, vm, on_threshold_changed, |vm, v| vm
        .set_threshold_percent(f64::from(v)));
    route!(w, vm, on_alert_provider_changed, |vm, id, on| vm
        .set_alert_provider(id.as_str(), on));
    route!(w, vm, on_quit, |vm| vm.quit());
    route!(w, vm, on_provider_enabled_changed, |vm, id, on| vm
        .set_provider_enabled(id.as_str(), on));
    route!(w, vm, on_move_provider, |vm, id, pos| vm
        .move_provider(id.as_str(), pos));
    route!(w, vm, on_show_selected_only_changed, |vm, v| vm
        .set_show_selected_only(v));
    route!(w, vm, on_graph_changed, |vm, id, on| vm
        .set_menu_bar_graph(id.as_str(), on));
    route!(w, vm, on_sign_in, |vm, id| vm.sign_in(id.as_str()));
    route!(w, vm, on_sign_out, |vm, id| vm.sign_out(id.as_str()));
    route!(w, vm, on_openrouter_sign_out, |vm| vm.sign_out_openrouter());
    route!(w, vm, on_category_changed, |vm, id, on| vm
        .set_product_visible(id.as_str(), on));
    route!(w, vm, on_clear_history, |vm| vm.clear_history());
    route!(w, vm, on_export_history, |vm, f| {
        if let Some(fmt) = ExportFormat::from_extension(f.as_str()) {
            vm.export_history(fmt);
        }
    });
    wire_openrouter_save(w, vm);
}

/// The save callback also clears the draft field when the key was accepted.
fn wire_openrouter_save(w: &Rc<SettingsWindow>, vm: &Arc<SettingsViewModel>) {
    let weak = w.as_weak();
    let vm = Arc::clone(vm);
    w.on_openrouter_save(move |draft| {
        let accepted = vm.save_openrouter_key(draft.as_str());
        if let Some(win) = weak.upgrade() {
            if accepted {
                win.set_openrouter_draft(SharedString::new());
            }
            apply_screen(&win, &vm.screen());
        }
    });
}
