//! The Windows app theme and dark title bars (Windows only).
//!
//! The mapping from the registry value to a theme is
//! [`tokenmon_core::AppTheme`]; this module is the thin OS adapter.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{apps_theme, set_dark_title_bar};
