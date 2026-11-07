//! The one panel window, behind a factory seam (issue #81).
//!
//! Every provider tab is a body inside a single window, so the shell asks
//! [`SinglePanel::get_or_create`] for "the" window and never builds a second
//! one. The factory is injected so portable tests can count creations without
//! a display; the Windows shell passes the real Slint window constructor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use tokenmon_core::ProviderId;

use crate::panel::PanelState;

/// Builds the panel window; the error is logged by the caller.
pub type WindowFactory<W> = Box<dyn Fn() -> Result<Rc<W>, String>>;

/// Owns at most one panel window, created lazily on first use.
pub struct SinglePanel<W> {
    factory: WindowFactory<W>,
    window: RefCell<Option<Rc<W>>>,
    creations: Cell<usize>,
}

impl<W> SinglePanel<W> {
    /// A holder that has not created its window yet.
    pub fn new(factory: WindowFactory<W>) -> Self {
        Self {
            factory,
            window: RefCell::new(None),
            creations: Cell::new(0),
        }
    }

    /// The window if it exists already; never creates one.
    pub fn existing(&self) -> Option<Rc<W>> {
        self.window.borrow().clone()
    }

    /// The window, created by the factory on the first call. A failed
    /// creation is retried on the next call and returns the error.
    ///
    /// # Errors
    ///
    /// The factory's error text when the window cannot be built.
    pub fn get_or_create(&self) -> Result<Rc<W>, String> {
        if let Some(w) = self.existing() {
            return Ok(w);
        }
        let w = (self.factory)()?;
        self.creations.set(self.creations.get() + 1);
        *self.window.borrow_mut() = Some(Rc::clone(&w));
        Ok(w)
    }

    /// How many windows this holder has created (at most one).
    pub fn creations(&self) -> usize {
        self.creations.get()
    }
}

/// Switches the panel to `provider`: records it as the selected tab and hands
/// it to `persist` (the `selectedProvider` setting, like the Mac), without
/// touching the window. Returns whether the tab changed.
pub fn select_tab(
    state: &mut PanelState,
    provider: ProviderId,
    persist: &dyn Fn(ProviderId),
) -> bool {
    let changed = state.selected != provider;
    state.selected = provider;
    persist(provider);
    changed
}
