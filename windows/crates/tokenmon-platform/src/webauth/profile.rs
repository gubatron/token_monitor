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

    /// Refuses roots that are not the dedicated sign-in folder
    /// (`<LOCALAPPDATA>\\TokenMon\\signin`): the final component must be
    /// `signin` under a `TokenMon` parent, so a wiring mistake (the TokenMon
    /// data dir, a drive root, the user profile) can never be deleted
    /// recursively, and the root itself must not be a symlink or junction
    /// that redirects the purge elsewhere.
    pub fn check_root(&self) -> io::Result<()> {
        let refuse = |why: &str| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("refusing to purge {}: {why}", self.root.display()),
            ))
        };
        let name_is = |p: Option<&std::ffi::OsStr>, want: &str| {
            p.and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(want))
        };
        if !name_is(self.root.file_name(), "signin") {
            return refuse("the final component is not `signin`");
        }
        if !name_is(self.root.parent().and_then(Path::file_name), "TokenMon") {
            return refuse("the parent is not `TokenMon`");
        }
        if self
            .root
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return refuse("the path contains `..`");
        }
        match std::fs::symlink_metadata(&self.root) {
            // `is_symlink` is also true for junctions (name-surrogate
            // reparse points) on Windows.
            Ok(m) if m.file_type().is_symlink() => refuse("the root is a symlink or junction"),
            Ok(m) if !m.is_dir() => refuse("the root is not a directory"),
            Ok(_) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Deletes one provider's folders; a missing folder is fine. Refused
    /// unless [`check_root`](Self::check_root) passes.
    pub fn purge_provider(&self, provider: &str) -> io::Result<()> {
        self.check_root()?;
        let dir = self
            .provider_dir(provider)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        remove_dir_if_exists(&dir)
    }

    /// Deletes every provider's folders (startup purge of crash leftovers).
    /// Refused unless [`check_root`](Self::check_root) passes.
    pub fn purge_all(&self) -> io::Result<()> {
        self.check_root()?;
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
