//! Launch-at-login state machine (spec 01 section 2, "Launch at login").
//!
//! The OS side (`HKCU\...\Run` plus `StartupApproved\Run`) lives behind
//! [`LoginItemService`] in the platform crate; this module holds the portable
//! logic so it is tested with a fake on every OS.

use std::sync::Arc;

/// What the OS reports for the login item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    /// Not registered to start at login.
    NotRegistered,
    /// Registered and allowed to run.
    Enabled,
    /// Registered but the user disabled it in Task Manager (StartupApproved).
    RequiresApproval,
}

/// Error from the OS login-item API; the reason is for logs only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("login item operation failed: {0}")]
pub struct LoginItemError(pub String);

/// OS access to the "start at login" registration.
pub trait LoginItemService: Send + Sync {
    /// The current registration state.
    fn status(&self) -> LoginItemStatus;
    /// Registers the app to start at login.
    fn register(&self) -> Result<(), LoginItemError>;
    /// Removes the registration.
    fn unregister(&self) -> Result<(), LoginItemError>;
}

/// The toggle shown in Settings: reflects the real OS state, never a stale one.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use tokenmon_core::settings::{LaunchAtLogin, LoginItemService, LoginItemStatus, LoginItemError};
///
/// struct Off;
/// impl LoginItemService for Off {
///     fn status(&self) -> LoginItemStatus { LoginItemStatus::NotRegistered }
///     fn register(&self) -> Result<(), LoginItemError> { Err(LoginItemError("denied".into())) }
///     fn unregister(&self) -> Result<(), LoginItemError> { Ok(()) }
/// }
/// let login = LaunchAtLogin::new(Arc::new(Off));
/// assert!(!login.set(true), "a failed register reverts to the real state");
/// ```
pub struct LaunchAtLogin {
    service: Arc<dyn LoginItemService>,
}

impl std::fmt::Debug for LaunchAtLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchAtLogin").finish_non_exhaustive()
    }
}

impl LaunchAtLogin {
    /// Wraps an OS service.
    pub fn new(service: Arc<dyn LoginItemService>) -> Self {
        Self { service }
    }

    /// `launchAtLogin`: on when enabled or awaiting approval.
    pub fn is_on(&self) -> bool {
        self.service.status() != LoginItemStatus::NotRegistered
    }

    /// `launchAtLoginNeedsApproval`.
    pub fn needs_approval(&self) -> bool {
        self.service.status() == LoginItemStatus::RequiresApproval
    }

    /// Registers or unregisters (errors are logged and swallowed), then returns
    /// the value the toggle must show, re-read from the OS.
    pub fn set(&self, on: bool) -> bool {
        let result = if on {
            self.service.register()
        } else {
            self.service.unregister()
        };
        if let Err(err) = result {
            tracing::warn!(error = %err, "launch at login change failed");
        }
        self.is_on()
    }

    /// Re-reads the OS state (called when the Settings window appears).
    pub fn refresh(&self) -> bool {
        self.is_on()
    }
}
