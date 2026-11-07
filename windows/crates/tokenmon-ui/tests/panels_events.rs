//! Panel callbacks reach the app as `PanelEvent`s.
#![allow(missing_docs, clippy::unwrap_used)]

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use tokenmon_core::ProviderId;
use tokenmon_ui::panels::events::{
    PanelEvent, PanelHandler, connect_chrome, connect_grok, connect_grokbot, connect_openrouter,
    connect_overview,
};
use tokenmon_ui::panels::ui;

struct TestPlatform;

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Every callback of several panels produces the expected event (one test:
/// Slint platform state is per process).
#[test]
fn test_panel_callbacks_emit_events() {
    slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
    let log: Rc<RefCell<Vec<PanelEvent>>> = Rc::default();
    let sink = log.clone();
    let handler: PanelHandler = Rc::new(move |e| sink.borrow_mut().push(e));

    let w = ui::PanelWindow::new().unwrap();
    connect_chrome(&w, &handler);
    let grok = w.global::<ui::GrokVm>();
    connect_grok(&grok, &handler);
    grok.invoke_open_settings();
    grok.invoke_sign_in();
    grok.invoke_sign_in_again();
    grok.invoke_sign_out();
    grok.invoke_previous_week();
    grok.invoke_next_week();
    w.invoke_select_tab(1, 2);

    let bot = w.global::<ui::GrokbotVm>();
    connect_grokbot(&bot, &handler);
    bot.invoke_sign_in();

    let or = w.global::<ui::OpenRouterVm>();
    connect_openrouter(&or, &handler);
    or.invoke_submit_key("sk-or-v1-x".into());
    or.invoke_replace_key();

    let ov = w.global::<ui::OverviewVm>();
    connect_overview(&ov, &handler);
    ov.invoke_card_sign_in(3);
    ov.invoke_previous_day();

    assert_eq!(
        *log.borrow(),
        vec![
            PanelEvent::OpenSettings,
            PanelEvent::SignIn(ProviderId::Grok),
            PanelEvent::SignInAgain(ProviderId::Grok),
            PanelEvent::SignOut(ProviderId::Grok),
            PanelEvent::PreviousPeriod(ProviderId::Grok),
            PanelEvent::NextPeriod(ProviderId::Grok),
            PanelEvent::SelectTab { row: 1, col: 2 },
            // Grokbot signs in through Cursor.
            PanelEvent::SignIn(ProviderId::Cursor),
            PanelEvent::SubmitApiKey("sk-or-v1-x".into()),
            PanelEvent::ReplaceApiKey,
            PanelEvent::OverviewSignIn(3),
            PanelEvent::PreviousPeriod(ProviderId::Overview),
        ]
    );
}
