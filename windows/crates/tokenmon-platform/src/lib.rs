//! OS services behind traits.
//!
//! Every service has a trait, a `cfg(windows)` implementation and an in-memory
//! fake so the rest of the workspace tests on macOS and Linux. This is the only
//! crate allowed to contain `unsafe` code.

pub mod install;
pub mod notify;
pub mod power;
pub mod secrets;
pub mod shell;
pub mod webauth;
