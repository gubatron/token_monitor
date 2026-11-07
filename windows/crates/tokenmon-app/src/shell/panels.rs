//! The one panel window (issue #81): every provider tab is a body inside a
//! single Slint window, so the shell treats a tab switch as "apply this view
//! to the window and select that body".

use std::rc::Rc;

use slint::ComponentHandle;
use tokenmon_ui::panels::switcher::RowVm;
use tokenmon_ui::panels::{bridge, events, ui};

use crate::panel::PanelView;

/// The panel's width in logical pixels (`PanelWindow` in the UI crate).
pub const PANEL_WIDTH_DIP: f32 = 420.0;

/// The panel window, whichever provider it shows.
pub struct PanelWindow {
    window: ui::PanelWindow,
}

impl PanelWindow {
    /// Creates the window, routes every body's actions to `handler` and the
    /// close button to `on_close`.
    pub fn create(
        handler: &events::PanelHandler,
        on_close: &Rc<dyn Fn()>,
    ) -> Result<Self, slint::PlatformError> {
        let window = ui::PanelWindow::new()?;
        events::connect_all(&window, handler);
        window.set_closable(true);
        let close = Rc::clone(on_close);
        window.on_close_panel(move || close());
        Ok(Self { window })
    }

    /// The Slint window.
    pub fn window(&self) -> &slint::Window {
        self.window.window()
    }

    /// Whether it is on screen.
    pub fn is_visible(&self) -> bool {
        self.window().is_visible()
    }

    /// Shows it.
    pub fn show(&self) {
        let _ = self.window.show();
    }

    /// The size the layout wants, in logical pixels. Read from the component,
    /// not from the window: before the first show the window's own size is not
    /// meaningful (it may be zero or at the wrong scale), which put the panel
    /// at the icon's centre and off-screen (#81).
    pub fn logical_size(&self) -> (f32, f32) {
        (PANEL_WIDTH_DIP, self.window.get_content_height())
    }

    /// The native window handle, once the window exists.
    pub fn hwnd(&self) -> Option<isize> {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        self.window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                _ => None,
            })
            .flatten()
    }

    /// Hides it.
    pub fn hide(&self) {
        let _ = self.window.hide();
    }

    /// Fills the window from `view` and the switcher grid and selects the
    /// view's body in place.
    pub fn apply(&self, view: &PanelView, rows: &[RowVm]) {
        let w = &self.window;
        w.set_tabs(bridge::tab_rows(rows));
        match view {
            PanelView::Overview(v) => bridge::apply_overview(&w.global::<ui::OverviewVm>(), v),
            PanelView::Grok(v) => bridge::apply_grok(&w.global::<ui::GrokVm>(), v),
            PanelView::Grokbot(v) => bridge::apply_grokbot(&w.global::<ui::GrokbotVm>(), v),
            PanelView::Cursor(v) => bridge::apply_cursor(&w.global::<ui::CursorVm>(), v),
            PanelView::Claude(v) => bridge::apply_claude(&w.global::<ui::ClaudeVm>(), v),
            PanelView::ChatGpt(v) => bridge::apply_chatgpt(&w.global::<ui::ChatGptVm>(), v),
            PanelView::OpenCode(v) => bridge::apply_opencode(&w.global::<ui::OpenCodeVm>(), v),
            PanelView::OpenRouter(v) => {
                bridge::apply_openrouter(&w.global::<ui::OpenRouterVm>(), v);
            }
        }
        w.set_selected(bridge::provider_tab(view.provider()));
    }
}
