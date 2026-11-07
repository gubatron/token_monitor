//! User actions raised by the panels, as plain Rust values.
//!
//! Each `connect_*` function wires a generated Slint panel to one
//! [`PanelHandler`]: the app (ws-app) supplies a single closure and matches on
//! [`PanelEvent`] to open the settings window, start a sign-in, page a chart,
//! and so on. The panels themselves never decide what an action does.
//!
//! Callbacks are connected once per window; the view model that fills the
//! panel can change as often as it likes. Where an action depends on the
//! current view model (Overview cards, switcher cells) the event carries the
//! raw index and the app resolves it against the model it last applied.

use std::rc::Rc;

use slint::ComponentHandle;

use tokenmon_core::ProviderId;

use super::ui;

/// One user action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelEvent {
    /// The gear in a panel header: open Settings.
    OpenSettings,
    /// A switcher cell was clicked; resolve with
    /// [`tab_provider`](super::switcher::tab_provider).
    SelectTab {
        /// Row of the 4-per-row grid.
        row: usize,
        /// Column within the row.
        col: usize,
    },
    /// A sign-in button: open this provider's sign-in window.
    SignIn(ProviderId),
    /// `Sign In Again…` for this provider.
    SignInAgain(ProviderId),
    /// The sign-out control was confirmed for this provider.
    SignOut(ProviderId),
    /// The week/day chart's left arrow (`Overview` pages days, others weeks).
    PreviousPeriod(ProviderId),
    /// The week/day chart's right arrow.
    NextPeriod(ProviderId),
    /// The collapsible models header was toggled.
    ToggleModelsSection(ProviderId),
    /// `View all` / `Show less` in a models card.
    ToggleShowAllModels(ProviderId),
    /// Overview card `index` sign-in button (resolve against the applied
    /// model's [`CardAction`](super::overview::CardAction)).
    OverviewSignIn(usize),
    /// Overview card `index` sign-out control.
    OverviewSignOut(usize),
    /// The OpenRouter key field changed; recompute the button state with
    /// [`key_button_enabled`](super::opencode_openrouter::key_button_enabled).
    ApiKeyEdited(String),
    /// The OpenRouter key was submitted (button or Enter).
    SubmitApiKey(String),
    /// `Replace API Key…` was clicked.
    ReplaceApiKey,
}

/// The single sink the app provides for all panel actions.
pub type PanelHandler = Rc<dyn Fn(PanelEvent)>;

fn index(v: i32) -> usize {
    usize::try_from(v).unwrap_or(0)
}

/// The gear every provider body shows in its header: open Settings.
macro_rules! common {
    ($w:expr, $h:expr) => {{
        let h = $h.clone();
        $w.on_open_settings(move || h(PanelEvent::OpenSettings));
    }};
}

/// Sign-in, sign-in-again and sign-out for a provider's usage panel.
macro_rules! auth {
    ($w:expr, $h:expr, $sign_in:expr, $sign_out:expr) => {{
        let (h, p) = ($h.clone(), $sign_in);
        $w.on_sign_in(move || h(PanelEvent::SignIn(p)));
        let (h, p) = ($h.clone(), $sign_in);
        $w.on_sign_in_again(move || h(PanelEvent::SignInAgain(p)));
        let (h, p) = ($h.clone(), $sign_out);
        $w.on_sign_out(move || h(PanelEvent::SignOut(p)));
    }};
}

/// Week arrows of a panel with a daily chart.
macro_rules! weeks {
    ($w:expr, $h:expr, $p:expr) => {{
        let (h, p) = ($h.clone(), $p);
        $w.on_previous_week(move || h(PanelEvent::PreviousPeriod(p)));
        let (h, p) = ($h.clone(), $p);
        $w.on_next_week(move || h(PanelEvent::NextPeriod(p)));
    }};
}

/// Connects the window chrome: the switcher cells. Call once per window.
pub fn connect_chrome(w: &ui::PanelWindow, handler: &PanelHandler) {
    let h = handler.clone();
    w.on_select_tab(move |row, col| {
        h(PanelEvent::SelectTab {
            row: index(row),
            col: index(col),
        });
    });
}

/// Connects every provider body of the one panel window. Call once per
/// window; the view models can then change as often as they like.
pub fn connect_all(w: &ui::PanelWindow, handler: &PanelHandler) {
    connect_chrome(w, handler);
    connect_overview(&w.global::<ui::OverviewVm>(), handler);
    connect_grok(&w.global::<ui::GrokVm>(), handler);
    connect_grokbot(&w.global::<ui::GrokbotVm>(), handler);
    connect_cursor(&w.global::<ui::CursorVm>(), handler);
    connect_claude(&w.global::<ui::ClaudeVm>(), handler);
    connect_chatgpt(&w.global::<ui::ChatGptVm>(), handler);
    connect_opencode(&w.global::<ui::OpenCodeVm>(), handler);
    connect_openrouter(&w.global::<ui::OpenRouterVm>(), handler);
}

/// Connects a Grok panel.
pub fn connect_grok(w: &ui::GrokVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    auth!(w, handler, ProviderId::Grok, ProviderId::Grok);
    weeks!(w, handler, ProviderId::Grok);
}

/// Connects a Grokbot panel (its sign-in and sign-out act on Cursor's session).
pub fn connect_grokbot(w: &ui::GrokbotVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    auth!(w, handler, ProviderId::Cursor, ProviderId::Cursor);
    weeks!(w, handler, ProviderId::Grokbot);
}

/// Connects a Cursor panel.
pub fn connect_cursor(w: &ui::CursorVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    auth!(w, handler, ProviderId::Cursor, ProviderId::Cursor);
    weeks!(w, handler, ProviderId::Cursor);
}

/// Connects a Claude panel (usage and Claude Code sections).
pub fn connect_claude(w: &ui::ClaudeVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    auth!(w, handler, ProviderId::Claude, ProviderId::Claude);
    weeks!(w, handler, ProviderId::Claude);
    let h = handler.clone();
    w.on_toggle_code_models(move || h(PanelEvent::ToggleModelsSection(ProviderId::Claude)));
    let h = handler.clone();
    w.on_toggle_show_all_models(move || h(PanelEvent::ToggleShowAllModels(ProviderId::Claude)));
}

/// Connects a ChatGPT panel (no chart, so no week arrows).
pub fn connect_chatgpt(w: &ui::ChatGptVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    auth!(w, handler, ProviderId::ChatGpt, ProviderId::ChatGpt);
}

/// Connects an OpenCode panel.
pub fn connect_opencode(w: &ui::OpenCodeVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    auth!(w, handler, ProviderId::OpenCode, ProviderId::OpenCode);
    weeks!(w, handler, ProviderId::OpenCode);
    let h = handler.clone();
    w.on_toggle_models(move || h(PanelEvent::ToggleModelsSection(ProviderId::OpenCode)));
    let h = handler.clone();
    w.on_toggle_show_all_models(move || {
        h(PanelEvent::ToggleShowAllModels(ProviderId::OpenCode));
    });
}

/// Connects an OpenRouter panel (API-key entry instead of a sign-in window).
pub fn connect_openrouter(w: &ui::OpenRouterVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    let h = handler.clone();
    w.on_sign_out(move || h(PanelEvent::SignOut(ProviderId::OpenRouter)));
    let h = handler.clone();
    w.on_key_edited(move |s| h(PanelEvent::ApiKeyEdited(s.to_string())));
    let h = handler.clone();
    w.on_submit_key(move |s| h(PanelEvent::SubmitApiKey(s.to_string())));
    let h = handler.clone();
    w.on_replace_key(move || h(PanelEvent::ReplaceApiKey));
    let h = handler.clone();
    w.on_toggle_show_all_models(move || {
        h(PanelEvent::ToggleShowAllModels(ProviderId::OpenRouter));
    });
}

/// Connects the Overview panel. Its arrows page the hourly chart by day.
pub fn connect_overview(w: &ui::OverviewVm<'_>, handler: &PanelHandler) {
    common!(w, handler);
    let h = handler.clone();
    w.on_card_sign_in(move |i| h(PanelEvent::OverviewSignIn(index(i))));
    let h = handler.clone();
    w.on_card_sign_out(move |i| h(PanelEvent::OverviewSignOut(index(i))));
    let h = handler.clone();
    w.on_previous_day(move || h(PanelEvent::PreviousPeriod(ProviderId::Overview)));
    let h = handler.clone();
    w.on_next_day(move || h(PanelEvent::NextPeriod(ProviderId::Overview)));
}
