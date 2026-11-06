//! Launch at login through `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
//!
//! Mirrors macOS `SMAppService.mainApp`: [`LoginItemService::status`] reports
//! `Enabled`, `RequiresApproval` (the user switched it off in Task Manager >
//! Startup apps) or `NotRegistered`. The Settings toggle calls
//! `register`/`unregister`, swallows errors and re-reads `status`.

use std::sync::Arc;

use crate::shell::command_line;
use crate::shell::error::ShellError;
use crate::shell::registry::Registry;

/// Registry key holding the per-user autostart commands.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Registry key where Task Manager records the user's enable/disable choice.
pub const STARTUP_APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Value name used under both keys.
pub const VALUE_NAME: &str = "TokenMon";
/// Flag appended to the autostart command so the login launch stays in the tray.
pub const BACKGROUND_FLAG: &str = "--background";

/// State of the launch-at-login registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoginItemStatus {
    /// No Run entry exists.
    NotRegistered,
    /// Registered and not disabled in Task Manager.
    Enabled,
    /// Registered, but the user disabled it in Task Manager > Startup apps.
    /// The UI shows "Disabled in Task Manager" with a link to `ms-settings:startupapps`.
    RequiresApproval,
}

/// Launch-at-login control. Implemented by [`RunKeyLoginItem`]; UI code can
/// substitute its own fake.
pub trait LoginItemService: Send + Sync {
    /// Registers (or re-registers, updating a stale path) the login item.
    fn register(&self) -> Result<(), ShellError>;
    /// Removes the login item and the Task Manager override; succeeds if absent.
    fn unregister(&self) -> Result<(), ShellError>;
    /// Reads the current state back from the OS.
    fn status(&self) -> Result<LoginItemStatus, ShellError>;
}

/// [`LoginItemService`] backed by the HKCU Run key.
pub struct RunKeyLoginItem {
    registry: Arc<dyn Registry>,
    exe: String,
}

impl RunKeyLoginItem {
    /// Creates the service for the given executable path (normally
    /// `std::env::current_exe()` as a string).
    #[must_use]
    pub fn new(registry: Arc<dyn Registry>, exe: impl Into<String>) -> Self {
        Self {
            registry,
            exe: exe.into(),
        }
    }

    /// The exact command line this service writes: `"<exe>" --background`.
    ///
    /// # Errors
    ///
    /// [`ShellError::InvalidExecutablePath`] for an unusable path.
    pub fn command(&self) -> Result<String, ShellError> {
        command_line::build(&self.exe, &[BACKGROUND_FLAG])
    }

    /// Whether the stored command points at a different executable than this
    /// one (the app was moved or reinstalled elsewhere). `false` when not registered.
    pub fn is_stale(&self) -> Result<bool, ShellError> {
        let stored = self.registry.get_string(RUN_KEY, VALUE_NAME)?;
        Ok(stored
            .as_deref()
            .and_then(command_line::executable)
            .is_some_and(|stored_exe| !stored_exe.eq_ignore_ascii_case(&self.exe)))
    }
}

/// Task Manager stores a 12-byte blob per entry. Even first byte (`0x02`,
/// `0x06`) means enabled, odd (`0x03`, `0x07`) means disabled by the user.
fn disabled_by_user(blob: &[u8]) -> bool {
    blob.first().is_some_and(|b| b & 1 == 1)
}

impl LoginItemService for RunKeyLoginItem {
    fn register(&self) -> Result<(), ShellError> {
        let command = self.command()?;
        // Skip the write when already correct: keeps register idempotent and
        // avoids touching the key on every Settings open.
        if self.registry.get_string(RUN_KEY, VALUE_NAME)?.as_deref() == Some(command.as_str()) {
            return Ok(());
        }
        self.registry.set_string(RUN_KEY, VALUE_NAME, &command)
    }

    fn unregister(&self) -> Result<(), ShellError> {
        self.registry.delete_value(RUN_KEY, VALUE_NAME)?;
        self.registry.delete_value(STARTUP_APPROVED_KEY, VALUE_NAME)
    }

    fn status(&self) -> Result<LoginItemStatus, ShellError> {
        if self.registry.get_string(RUN_KEY, VALUE_NAME)?.is_none() {
            return Ok(LoginItemStatus::NotRegistered);
        }
        let blob = self.registry.get_binary(STARTUP_APPROVED_KEY, VALUE_NAME)?;
        Ok(match blob {
            Some(b) if disabled_by_user(&b) => LoginItemStatus::RequiresApproval,
            _ => LoginItemStatus::Enabled,
        })
    }
}
