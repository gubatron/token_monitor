//! First-run registration of the AppUserModelID under HKCU.
//!
//! An unpackaged app's toasts only show (with the right name and icon) when
//! its AUMID is known to the shell. The installer creates a Start-menu
//! shortcut carrying the AUMID; this registration at first run makes toasts
//! work even for a portable copy or a missing shortcut. Only HKCU is written,
//! so no elevation is needed.

use std::sync::Arc;

use crate::notify::NotifyError;
use crate::notify::toast::APP_USER_MODEL_ID;
use crate::shell::Registry;

/// Parent key of all AUMID registrations, relative to HKCU.
pub const AUMID_ROOT: &str = r"Software\Classes\AppUserModelId";
/// Name shown as the toast's app name.
pub const DISPLAY_NAME: &str = "TokenMon";

/// What [`AumidRegistration::ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AumidStatus {
    /// The registration was missing or stale and has been written.
    Registered,
    /// The registration was already correct; nothing was written.
    AlreadyRegistered,
}

/// Idempotent AUMID registration.
pub struct AumidRegistration {
    registry: Arc<dyn Registry>,
    icon_path: Option<String>,
}

impl AumidRegistration {
    /// `icon_path` is the absolute path of an icon (`.ico`/`.png`) shown on
    /// toasts, when known.
    pub fn new(registry: Arc<dyn Registry>, icon_path: Option<String>) -> Self {
        Self {
            registry,
            icon_path,
        }
    }

    /// The full key, relative to HKCU.
    #[must_use]
    pub fn key() -> String {
        format!(r"{AUMID_ROOT}\{APP_USER_MODEL_ID}")
    }

    /// Registers the AUMID if it is not registered with the desired values.
    pub fn ensure(&self) -> Result<AumidStatus, NotifyError> {
        let key = Self::key();
        let name_ok =
            self.registry.get_string(&key, "DisplayName")?.as_deref() == Some(DISPLAY_NAME);
        let icon_ok = match &self.icon_path {
            Some(p) => self.registry.get_string(&key, "IconUri")?.as_deref() == Some(p.as_str()),
            None => true,
        };
        if name_ok && icon_ok {
            return Ok(AumidStatus::AlreadyRegistered);
        }
        self.registry
            .set_string(&key, "DisplayName", DISPLAY_NAME)?;
        if let Some(p) = &self.icon_path {
            self.registry.set_string(&key, "IconUri", p)?;
        }
        Ok(AumidStatus::Registered)
    }

    /// Removes the registration (uninstall path); succeeds when absent.
    pub fn remove(&self) -> Result<(), NotifyError> {
        Ok(self.registry.delete_key_tree(&Self::key())?)
    }
}
