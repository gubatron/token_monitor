//! The eight panel windows behind one type, so the shell treats a tab switch
//! as "apply this view to that window".

use std::rc::Rc;

use slint::ComponentHandle;
use tokenmon_core::ProviderId;
use tokenmon_ui::panels::switcher::RowVm;
use tokenmon_ui::panels::{bridge, events, ui};

use crate::panel::PanelView;

/// One panel window, whichever provider it shows.
pub enum PanelWindow {
    /// Overview.
    Overview(ui::OverviewPanel),
    /// Grok.
    Grok(ui::GrokPanel),
    /// Grokbot.
    Grokbot(ui::GrokbotPanel),
    /// Cursor.
    Cursor(ui::CursorPanel),
    /// Claude.
    Claude(ui::ClaudePanel),
    /// ChatGPT.
    ChatGpt(ui::ChatGptPanel),
    /// OpenCode.
    OpenCode(ui::OpenCodePanel),
    /// OpenRouter.
    OpenRouter(ui::OpenRouterPanel),
}

/// Runs `$body` with `$w` bound to the inner window, whichever variant.
macro_rules! each {
    ($self:expr, $w:ident => $body:expr) => {
        match $self {
            PanelWindow::Overview($w) => $body,
            PanelWindow::Grok($w) => $body,
            PanelWindow::Grokbot($w) => $body,
            PanelWindow::Cursor($w) => $body,
            PanelWindow::Claude($w) => $body,
            PanelWindow::ChatGpt($w) => $body,
            PanelWindow::OpenCode($w) => $body,
            PanelWindow::OpenRouter($w) => $body,
        }
    };
}

impl PanelWindow {
    /// Creates the window for `provider`, routes its actions to `handler` and
    /// its close button to `on_close`.
    pub fn create(
        provider: ProviderId,
        handler: &events::PanelHandler,
        on_close: &Rc<dyn Fn()>,
    ) -> Result<Self, slint::PlatformError> {
        let window = match provider {
            ProviderId::Overview => {
                let w = ui::OverviewPanel::new()?;
                events::connect_overview(&w, handler);
                Self::Overview(w)
            }
            ProviderId::Grok => {
                let w = ui::GrokPanel::new()?;
                events::connect_grok(&w, handler);
                Self::Grok(w)
            }
            ProviderId::Grokbot => {
                let w = ui::GrokbotPanel::new()?;
                events::connect_grokbot(&w, handler);
                Self::Grokbot(w)
            }
            ProviderId::Cursor => {
                let w = ui::CursorPanel::new()?;
                events::connect_cursor(&w, handler);
                Self::Cursor(w)
            }
            ProviderId::Claude => {
                let w = ui::ClaudePanel::new()?;
                events::connect_claude(&w, handler);
                Self::Claude(w)
            }
            ProviderId::ChatGpt => {
                let w = ui::ChatGptPanel::new()?;
                events::connect_chatgpt(&w, handler);
                Self::ChatGpt(w)
            }
            ProviderId::OpenCode => {
                let w = ui::OpenCodePanel::new()?;
                events::connect_opencode(&w, handler);
                Self::OpenCode(w)
            }
            ProviderId::OpenRouter => {
                let w = ui::OpenRouterPanel::new()?;
                events::connect_openrouter(&w, handler);
                Self::OpenRouter(w)
            }
        };
        each!(&window, w => {
            w.set_closable(true);
            let close = Rc::clone(on_close);
            w.on_close_panel(move || close());
        });
        Ok(window)
    }

    /// The Slint window.
    pub fn window(&self) -> &slint::Window {
        each!(self, w => w.window())
    }

    /// Whether it is on screen.
    pub fn is_visible(&self) -> bool {
        self.window().is_visible()
    }

    /// Shows it.
    pub fn show(&self) {
        let _ = each!(self, w => w.show());
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
        let _ = each!(self, w => w.hide());
    }

    /// Fills the window from `view` and the switcher grid. A view of another
    /// provider's kind is ignored.
    pub fn apply(&self, view: &PanelView, rows: &[RowVm]) {
        let tabs = bridge::tab_rows(rows);
        match (self, view) {
            (Self::Overview(w), PanelView::Overview(v)) => {
                w.set_tabs(tabs);
                bridge::apply_overview(w, v);
            }
            (Self::Grok(w), PanelView::Grok(v)) => {
                w.set_tabs(tabs);
                bridge::apply_grok(w, v);
            }
            (Self::Grokbot(w), PanelView::Grokbot(v)) => {
                w.set_tabs(tabs);
                bridge::apply_grokbot(w, v);
            }
            (Self::Cursor(w), PanelView::Cursor(v)) => {
                w.set_tabs(tabs);
                bridge::apply_cursor(w, v);
            }
            (Self::Claude(w), PanelView::Claude(v)) => {
                w.set_tabs(tabs);
                bridge::apply_claude(w, v);
            }
            (Self::ChatGpt(w), PanelView::ChatGpt(v)) => {
                w.set_tabs(tabs);
                bridge::apply_chatgpt(w, v);
            }
            (Self::OpenCode(w), PanelView::OpenCode(v)) => {
                w.set_tabs(tabs);
                bridge::apply_opencode(w, v);
            }
            (Self::OpenRouter(w), PanelView::OpenRouter(v)) => {
                w.set_tabs(tabs);
                bridge::apply_openrouter(w, v);
            }
            _ => {}
        }
    }
}
