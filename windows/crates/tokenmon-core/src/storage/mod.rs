//! `AppPaths`, atomic owner-only writes, `FileBackedStringStore`, hourly/daily delta stores and `QuotaWindow`.
//!
//! Owner: ws-core-storage. Implements spec 01 section 1.3.
//!
//! The on-disk JSON shapes are structurally identical to the macOS app's (dates
//! are Apple reference-date seconds, `days` is a flat `[ref-seconds, value, ...]`
//! array), so a future import of macOS data stays possible.
//!
//! * [`AppPaths`]: the TokenMon data directory (`%LOCALAPPDATA%\TokenMon` in
//!   production, an injected base directory in tests).
//! * [`OwnerOnlyPermissions`]: the hook that restricts a directory/file to the
//!   current user.
//! * [`write_atomic`]: staging-file write plus rename.
//! * [`TextStore`] / [`FileBackedStringStore`] / [`InMemoryStringStore`]:
//!   per-key text files.
//! * [`HourlyDeltaActivityStore`], [`DailyQuotaDeltaStore`]: percentage-point
//!   growth stores.
//! * [`QuotaWindow`], [`QuotaWindowTransition`]: provider-reported quota window
//!   bounds.

mod atomic;
mod calendar;
mod daily;
mod delta;
mod hourly;
mod numfmt;
mod paths;
mod permissions;
mod quota;
mod string_store;

pub use atomic::write_atomic;
pub use calendar::{
    APPLE_EPOCH_UNIX_SECS, add_days_to_key, day_key, from_apple_seconds,
    key_for_stored_start_of_day, start_of_day, to_apple_seconds,
};
pub use daily::{DailyPayload, DailyQuotaDeltaStore, RETENTION_DAYS, decode_daily_payload};
pub use delta::{NOISE_FLOOR, RESET_DROP_FLOOR, credited_delta};
pub use hourly::{HourlyDeltaActivityStore, HourlyPayload, decode_hourly_payload};
pub use paths::{
    AppPaths, DIRECTORY_NAME, LEGACY_DIRECTORY_NAME, PathsError, home_directory_from_env,
    migrate_legacy_directory_if_needed,
};
pub use permissions::{NoopPermissions, OwnerOnlyPermissions, PlatformPermissions, icacls_args};
pub use quota::{DEFAULT_PERIOD_DAYS, QuotaWindow, QuotaWindowTransition};
pub use string_store::{FileBackedStringStore, InMemoryStringStore, TextStore};
