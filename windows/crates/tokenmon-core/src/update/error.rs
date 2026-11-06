//! Typed errors for the update check and the install flow.

use thiserror::Error;

/// A failure while checking GitHub for a newer release.
///
/// `Display` carries the user-facing "Update check failed: " prefix.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::UpdateError;
///
/// assert_eq!(UpdateError::from_status(403), UpdateError::RateLimited);
/// assert_eq!(
///     UpdateError::from_status(500).to_string(),
///     "Update check failed: HTTP 500"
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UpdateError {
    /// Transport failure (DNS, TLS, timeout).
    #[error("Update check failed: {0}")]
    Network(String),
    /// The server answered with something unusable.
    #[error("Update check failed: {0}")]
    BadResponse(String),
    /// HTTP 403 from GitHub, which in practice means the unauthenticated rate limit.
    #[error("Update check failed: GitHub rate limit reached; will retry later")]
    RateLimited,
}

impl UpdateError {
    /// Maps a non-2xx HTTP status to the matching error.
    pub fn from_status(status: u16) -> Self {
        if status == 403 {
            Self::RateLimited
        } else {
            Self::BadResponse(format!("HTTP {status}"))
        }
    }
}

/// A failure while downloading, verifying or installing an update.
///
/// Every variant's `Display` is safe to show to the user.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::InstallError;
///
/// assert_eq!(
///     InstallError::NotWritable.to_string(),
///     "TokenMon cannot replace itself from this folder."
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstallError {
    /// The download failed or was refused.
    #[error("Could not download the update: {0}")]
    Download(String),
    /// The archive could not be read or was refused.
    #[error("Could not read the update: {0}")]
    Archive(String),
    /// Checksum or signature verification failed.
    #[error("The update failed verification: {0}")]
    Verification(String),
    /// Staging, swapping or launching failed.
    #[error("Could not install the update: {0}")]
    Install(String),
    /// The running executable cannot be replaced from where it lives.
    #[error("TokenMon cannot replace itself from this folder.")]
    NotWritable,
}
