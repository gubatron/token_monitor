//! Issue #81: every provider tab is a body inside the one panel window, so
//! switching tabs never creates a second window.
#![allow(missing_docs, clippy::unwrap_used)]

use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use tokenmon_core::ProviderId;
use tokenmon_ui::panels::bridge::provider_tab;
use tokenmon_ui::panels::ui;

struct CountingPlatform(Rc<Cell<usize>>);

impl Platform for CountingPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        self.0.set(self.0.get() + 1);
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Selecting each of the eight providers on one `PanelWindow` creates exactly
/// one window adapter, and the selected tab is the one that was set.
#[test]
fn test_switching_every_tab_creates_no_second_window() {
    let created = Rc::new(Cell::new(0));
    slint::platform::set_platform(Box::new(CountingPlatform(created.clone()))).unwrap();
    let w = ui::PanelWindow::new().unwrap();
    w.show().unwrap();
    for provider in ProviderId::ALL_CASES {
        w.set_selected(provider_tab(provider));
        assert_eq!(w.get_selected(), provider_tab(provider));
        assert!(w.get_content_height() > 0.0);
    }
    assert_eq!(created.get(), 1, "one window for all tabs");
}
