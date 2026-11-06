//! The decision `main` makes at launch: become the primary instance, or hand
//! the request to the one that is already running.

use crate::shell::cli::LaunchArgs;
use crate::shell::error::ShellError;
use crate::shell::instance::{InstanceGuard, InstanceLock, InstanceState};
use crate::shell::ipc::{IpcClient, IpcMessage};

/// What this process should do after [`decide_startup`].
#[derive(Debug)]
pub enum StartupDecision {
    /// Run the app; hold `guard` until exit. `show_panel` says whether to open
    /// the panel immediately (`--show`, or a normal non-background launch).
    RunPrimary {
        /// Keeps the single-instance lock alive.
        guard: InstanceGuard,
        /// Whether to show the panel on startup.
        show_panel: bool,
    },
    /// Another instance is running and has been told to show its panel (or
    /// nothing needed to be shown); this process should exit quietly.
    HandedOff,
    /// Another instance is running but could not be reached. Exit with a
    /// message rather than start a second tray icon.
    HandoffFailed(ShellError),
}

/// Applies the launch rules:
///
/// * first instance: run, showing the panel unless `--background` (and no `--show`);
/// * second instance: if the panel was requested, send [`IpcMessage::Show`] to
///   the running one; either way exit.
///
/// # Errors
///
/// Only when the single-instance primitive itself fails.
pub fn decide_startup(
    args: LaunchArgs,
    lock: &dyn InstanceLock,
    ipc: &dyn IpcClient,
) -> Result<StartupDecision, ShellError> {
    match lock.try_acquire()? {
        InstanceState::Primary(guard) => Ok(StartupDecision::RunPrimary {
            guard,
            show_panel: args.wants_panel(),
        }),
        InstanceState::Secondary if !args.wants_panel() => Ok(StartupDecision::HandedOff),
        InstanceState::Secondary => Ok(match ipc.send(&IpcMessage::Show) {
            Ok(()) => StartupDecision::HandedOff,
            Err(e) => StartupDecision::HandoffFailed(e),
        }),
    }
}
