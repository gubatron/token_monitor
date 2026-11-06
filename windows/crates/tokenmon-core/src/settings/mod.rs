//! `AppSettings` model, clamps, `needs_polling` and JSON persistence (spec 01
//! section 2).
//!
//! * [`Settings`]: the immutable value with defaults, clamps, derived values and
//!   `with_*` updates. Pure, no I/O.
//! * [`SettingsFile`]: the atomic JSON object that replaces `UserDefaults`.
//! * [`AppSettings`]: the live handle; persists changes and broadcasts a new
//!   [`Settings`] snapshot on a `tokio::sync::watch` channel.
//!
//! Owner: ws-settings.

mod app;
mod file;
mod login_item;
mod model;

pub use app::AppSettings;
pub use file::{SETTINGS_FILE_NAME, SettingsFile, SettingsMap, decode_map};
pub use login_item::{LaunchAtLogin, LoginItemError, LoginItemService, LoginItemStatus};
pub use model::{
    ACTIVE_POLL_RANGE, DEFAULT_ACTIVE_POLL_SECONDS, DEFAULT_IDLE_POLL_SECONDS,
    DEFAULT_THRESHOLD_PERCENT, GrokProduct, IDLE_POLL_RANGE, InvalidEnabledProviders,
    LEGACY_GRAPH_FLAGS, PanelSection, ParseSettingsEnumError, Settings, SettingsKey,
    THRESHOLD_RANGE, migrate_graph_flags,
};

/// Decodes raw `settings.json` bytes into a valid [`Settings`]. Never fails or
/// panics: corrupt input yields clamped defaults. This is the fuzzed entry point.
///
/// # Examples
///
/// ```
/// use tokenmon_core::settings::{decode_settings, Settings};
///
/// assert_eq!(decode_settings(b"\x00garbage"), Settings::default());
/// ```
pub fn decode_settings(bytes: &[u8]) -> Settings {
    Settings::from_map(&decode_map(bytes))
}
