//! TokenMon for Windows: the composition root.
//!
//! Everything the executable does lives in this library so it can be driven
//! hermetically with fakes: the [`registry`] of providers, the [`model`] that
//! owns lifecycle behaviour (menu open, sign-in completion, wake handling,
//! settings-driven polling, quit), [`alerts`] for threshold notifications and
//! [`smoke`] for the headless CI run. `main.rs` only parses the command line
//! and hands control to the platform glue.

pub mod alerts;
pub mod app;
pub mod daily;
pub mod flush;
pub mod model;
pub mod observe;
pub mod overview;
pub mod panel;
pub mod poller;
pub mod providers;
pub mod registry;
pub mod sessions;
pub mod settings_host;
pub mod signin;
pub mod smoke;
pub mod sources;
pub mod testing;
pub mod tray;
pub mod updates;

#[cfg(windows)]
pub mod shell;
