//! TokenMon provider: OpenCode (Go subscription and Zen).
//!
//! * [`console`]: console client over an injected transport.
//! * [`db`]: read-only `opencode.db` reader, path probing and scan cache.
//! * [`stats`]: local estimate (snapshot, hourly stacks, daily spends).
//! * [`pricing`] and [`model_name`]: price tables and model naming.
//! * [`bounds`]: week and subscription-month windows.
//! * [`auth`]: sign-in policy.
//!
//! Owner: ws-opencode. The polling wrapper (`OpenCodeUsagePoller`) is wired
//! by ws-app once the core polling loop lands.

pub mod auth;
pub mod bounds;
pub mod console;
pub mod db;
pub mod model_name;
pub mod pricing;
pub mod snapshot;
pub mod stats;
mod tables;

/// Caption strings shown when the console is unavailable (spec 03 section 3.10).
pub mod captions {
    /// No cookie at all.
    pub const NO_COOKIE: &str =
        "Showing local estimate. Sign in to OpenCode for official Go usage.";
    /// Session invalidated after repeated 401/403.
    pub const SESSION_EXPIRED: &str = "Console session expired \u{2014} showing local estimate. Sign in again for official numbers.";
    /// Any other console failure.
    pub const FETCH_FAILED: &str = "Console fetch failed \u{2014} showing local estimate.";
    /// Local database could not be read.
    pub const LOCAL_UNREADABLE: &str = "Could not read local OpenCode usage.";
    /// Panel caption under the bars when estimated.
    pub const ESTIMATED_BAR: &str =
        "Estimated from local sessions \u{2014} sign in for console accuracy.";
    /// Data source label for console data.
    pub const SOURCE_CONSOLE: &str = "OpenCode console";
    /// Data source label for local data.
    pub const SOURCE_LOCAL: &str = "Local estimate";

    /// Caption for the local-fallback state.
    pub fn fallback_error(has_cookie: bool, needs_sign_in: bool) -> &'static str {
        if !has_cookie {
            NO_COOKIE
        } else if needs_sign_in {
            SESSION_EXPIRED
        } else {
            FETCH_FAILED
        }
    }
}
