//! The remaining panels' callbacks reach the app as `PanelEvent`s, including
//! the edge cases of index conversion and the shared provider mapping.
#![allow(missing_docs, clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use tokenmon_core::ProviderId;
use tokenmon_ui::panels::events::{
    PanelEvent, PanelHandler, connect_chatgpt, connect_claude, connect_cursor, connect_opencode,
    connect_openrouter, connect_overview,
};
use tokenmon_ui::panels::ui;

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Cursor, Claude, ChatGPT, OpenCode, OpenRouter and Overview callbacks each
/// produce the event for their own provider (one test: Slint platform state is
/// per process).
#[test]
fn test_remaining_panel_callbacks_emit_events() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let log: Rc<RefCell<Vec<PanelEvent>>> = Rc::default();
    let sink = log.clone();
    let handler: PanelHandler = Rc::new(move |e| sink.borrow_mut().push(e));

    let cursor = ui::CursorPanel::new().unwrap();
    connect_cursor(&cursor, &handler);
    cursor.invoke_sign_in();
    cursor.invoke_sign_in_again();
    cursor.invoke_sign_out();
    cursor.invoke_previous_week();
    cursor.invoke_next_week();
    // A negative cell index from the UI must not wrap around.
    cursor.invoke_select_tab(-1, 3);

    let claude = ui::ClaudePanel::new().unwrap();
    connect_claude(&claude, &handler);
    claude.invoke_sign_in();
    claude.invoke_toggle_code_models();
    claude.invoke_toggle_show_all_models();
    claude.invoke_next_week();

    let gpt = ui::ChatGptPanel::new().unwrap();
    connect_chatgpt(&gpt, &handler);
    gpt.invoke_sign_out();
    gpt.invoke_open_settings();

    let oc = ui::OpenCodePanel::new().unwrap();
    connect_opencode(&oc, &handler);
    oc.invoke_sign_in_again();
    oc.invoke_toggle_models();
    oc.invoke_toggle_show_all_models();
    oc.invoke_previous_week();

    let or = ui::OpenRouterPanel::new().unwrap();
    connect_openrouter(&or, &handler);
    or.invoke_sign_out();
    or.invoke_key_edited("sk-or".into());
    or.invoke_toggle_show_all_models();

    let ov = ui::OverviewPanel::new().unwrap();
    connect_overview(&ov, &handler);
    ov.invoke_card_sign_out(2);
    ov.invoke_card_sign_in(-4);
    ov.invoke_next_day();

    let c = ProviderId::Cursor;
    assert_eq!(
        *log.borrow(),
        vec![
            PanelEvent::SignIn(c),
            PanelEvent::SignInAgain(c),
            PanelEvent::SignOut(c),
            PanelEvent::PreviousPeriod(c),
            PanelEvent::NextPeriod(c),
            PanelEvent::SelectTab { row: 0, col: 3 },
            PanelEvent::SignIn(ProviderId::Claude),
            PanelEvent::ToggleModelsSection(ProviderId::Claude),
            PanelEvent::ToggleShowAllModels(ProviderId::Claude),
            PanelEvent::NextPeriod(ProviderId::Claude),
            PanelEvent::SignOut(ProviderId::ChatGpt),
            PanelEvent::OpenSettings,
            PanelEvent::SignInAgain(ProviderId::OpenCode),
            PanelEvent::ToggleModelsSection(ProviderId::OpenCode),
            PanelEvent::ToggleShowAllModels(ProviderId::OpenCode),
            PanelEvent::PreviousPeriod(ProviderId::OpenCode),
            PanelEvent::SignOut(ProviderId::OpenRouter),
            PanelEvent::ApiKeyEdited("sk-or".into()),
            PanelEvent::ToggleShowAllModels(ProviderId::OpenRouter),
            PanelEvent::OverviewSignOut(2),
            PanelEvent::OverviewSignIn(0),
            PanelEvent::NextPeriod(ProviderId::Overview),
        ]
    );
}
