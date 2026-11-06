//! Opt-in "Show TokenMon" entry in the desktop-background right-click menu.
//!
//! Written under `HKCU\Software\Classes\DesktopBackground\Shell\TokenMon`
//! (per-user, no elevation). Windows 11 shows classic-verb entries only under
//! "Show more options"; a modern-menu entry would need a sparse package
//! identity plus an `IExplorerCommand` COM server, which is out of scope here
//! (tracked as a maintainer question on the issue).

use std::sync::Arc;

use crate::shell::command_line;
use crate::shell::error::ShellError;
use crate::shell::registry::Registry;

/// Verb key, relative to HKCU.
pub const VERB_KEY: &str = r"Software\Classes\DesktopBackground\Shell\TokenMon";
/// Command subkey, relative to HKCU.
pub const COMMAND_KEY: &str = r"Software\Classes\DesktopBackground\Shell\TokenMon\command";
/// Menu caption.
pub const CAPTION: &str = "Show TokenMon";
/// Flag the verb passes; the running instance (or a fresh one) shows the panel.
pub const SHOW_FLAG: &str = "--show";

/// Install/remove control for the Explorer verb.
pub trait ShellVerbService: Send + Sync {
    /// Adds (or repairs) the verb; idempotent.
    fn install(&self) -> Result<(), ShellError>;
    /// Removes the verb and its subkeys; succeeds if absent.
    fn uninstall(&self) -> Result<(), ShellError>;
    /// Whether the verb is currently present.
    fn is_installed(&self) -> Result<bool, ShellError>;
}

/// The desktop-background verb, backed by a [`Registry`].
pub struct DesktopVerb {
    registry: Arc<dyn Registry>,
    exe: String,
}

impl DesktopVerb {
    /// Creates the service for the given executable path.
    #[must_use]
    pub fn new(registry: Arc<dyn Registry>, exe: impl Into<String>) -> Self {
        Self {
            registry,
            exe: exe.into(),
        }
    }

    /// The command the verb runs: `"<exe>" --show`.
    ///
    /// # Errors
    ///
    /// [`ShellError::InvalidExecutablePath`] for an unusable path.
    pub fn command(&self) -> Result<String, ShellError> {
        command_line::build(&self.exe, &[SHOW_FLAG])
    }

    fn icon(&self) -> Result<String, ShellError> {
        Ok(format!("{},0", command_line::quote_exe(&self.exe)?))
    }
}

impl ShellVerbService for DesktopVerb {
    fn install(&self) -> Result<(), ShellError> {
        // Validate first so a bad path never leaves a half-written verb.
        let command = self.command()?;
        let icon = self.icon()?;
        self.registry.set_string(VERB_KEY, "", CAPTION)?;
        self.registry.set_string(VERB_KEY, "Icon", &icon)?;
        self.registry.set_string(COMMAND_KEY, "", &command)
    }

    fn uninstall(&self) -> Result<(), ShellError> {
        self.registry.delete_key_tree(VERB_KEY)
    }

    fn is_installed(&self) -> Result<bool, ShellError> {
        Ok(self.registry.get_string(COMMAND_KEY, "")?.is_some())
    }
}
