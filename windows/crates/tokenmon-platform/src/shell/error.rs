//! Error type shared by the shell services.

use thiserror::Error;

/// Failure of a shell integration operation (registry, mutex, pipe).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ShellError {
    /// The registry rejected an operation (Win32 error code in the message).
    #[error("registry operation failed: {0}")]
    Registry(String),
    /// The executable path cannot be placed in a command line safely
    /// (empty, or contains `"`, NUL or a line break).
    #[error("executable path is not usable in a command line: {0:?}")]
    InvalidExecutablePath(String),
    /// The single-instance primitive failed for a reason other than "already running".
    #[error("single instance check failed: {0}")]
    Instance(String),
    /// Delivering a message to the running instance failed.
    #[error("could not reach the running TokenMon instance: {0}")]
    Ipc(String),
}
