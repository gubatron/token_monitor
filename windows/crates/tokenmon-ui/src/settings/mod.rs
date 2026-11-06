//! Settings window: typed adapters ([`model`]), display strings ([`screen`]),
//! intents ([`view_model`]), in-memory fakes ([`fake`]) and the Slint binding
//! ([`bind`]). See spec 01 section 2.1.

#![allow(clippy::struct_excessive_bools)]

pub mod bind;
pub mod fake;
pub mod model;
pub mod screen;
pub mod view_model;

pub use model::{
    AccountStatus, ExportFormat, HostStatus, NativeSaveDialog, OpenRouterStatus, ProductCategory,
    RefreshStatus, SaveDialog, SettingsBackend, SettingsChange, SettingsHost, SettingsSnapshot,
};
pub use view_model::{SettingsViewModel, Step};
