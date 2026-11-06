//! Slint components and plain-Rust view models.
//!
//! View models are ordinary Rust types, unit-tested without a window. The
//! `.slint` sources live under `ui/`: `ui/components/` (ws-ui-kit),
//! `ui/panels/` (ws-ui-panels) and `ui/settings/` (ws-ui-settings).

pub mod panels;
pub mod settings;
pub mod tray_icon;
