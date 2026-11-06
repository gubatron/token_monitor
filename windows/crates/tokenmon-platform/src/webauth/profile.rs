//! Per-provider isolated, non-persistent WebView2 user-data folders.
//!
//! macOS uses an isolated data store per provider; on Windows that is a
//! separate user-data folder, so cookies are never shared across providers.
//! The folder only lives for one sign-in session and is deleted after capture
//! (or on cancel), and leftovers from a crash are purged at startup, so the
//! browser profile (history, cache, cookies) is never persisted.

use std::io;
use std::path::{Path, PathBuf};

/// A provider slug or session token that is unsafe as a path component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid profile name {0:?}: use 1-64 characters of a-z, 0-9, '-' or '_'")]
pub struct InvalidProfileName(pub String);

fn is_valid_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Root of all sign-in profiles (`%LOCALAPPDATA%\TokenMon\signin`).
#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    /// A store rooted at `root` (created lazily).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/<provider>`: everything of one provider.
    pub fn provider_dir(&self, provider: &str) -> Result<PathBuf, InvalidProfileName> {
        if !is_valid_name(provider) {
            return Err(InvalidProfileName(provider.to_owned()));
        }
        Ok(self.root.join(provider))
    }

    /// `<root>/<provider>/<token>`: the user-data folder for one sign-in
    /// session; `token` makes concurrent or retried sessions disjoint (a
    /// previous session's browser may still hold its folder).
    pub fn session_dir(&self, provider: &str, token: &str) -> Result<PathBuf, InvalidProfileName> {
        if !is_valid_name(token) {
            return Err(InvalidProfileName(token.to_owned()));
        }
        Ok(self.provider_dir(provider)?.join(token))
    }

    /// Deletes one provider's folders; a missing folder is fine.
    pub fn purge_provider(&self, provider: &str) -> io::Result<()> {
        let dir = self
            .provider_dir(provider)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        remove_dir_if_exists(&dir)
    }

    /// Deletes every provider's folders (startup purge of crash leftovers).
    pub fn purge_all(&self) -> io::Result<()> {
        remove_dir_if_exists(&self.root)
    }
}

/// Removes `dir` recursively. A missing directory is success. Symlinked
/// directories are removed as links, never followed.
pub fn remove_dir_if_exists(dir: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => {
            std::fs::remove_file(dir).or_else(|_| std::fs::remove_dir(dir))
        }
        Ok(_) => std::fs::remove_dir_all(dir),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
