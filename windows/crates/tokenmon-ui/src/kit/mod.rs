//! View-model helpers for the Slint UI kit (ws-ui-kit).
//!
//! Slint cannot format numbers or iterate models in expressions, so these
//! pure functions prepare the strings and geometry the components display.
//! Everything here is unit-tested without a window.

pub mod daily;
pub mod hover;
pub mod palette;
pub mod segments;
pub mod ui;
pub mod week;
