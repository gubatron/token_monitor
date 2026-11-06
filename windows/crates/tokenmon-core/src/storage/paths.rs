//! `AppPaths`: where TokenMon keeps its files.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::permissions::{OwnerOnlyPermissions, PlatformPermissions};
use super::string_store::FileBackedStringStore;

/// Name of the TokenMon data folder.
pub const DIRECTORY_NAME: &str = "TokenMon";
/// Name of the pre-rename folder, migrated on first use.
pub const LEGACY_DIRECTORY_NAME: &str = "ModelMonitor";

/// Errors resolving the base directory.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathsError {
    /// Neither `LOCALAPPDATA` nor a fallback location is available.
    #[error("no local application data directory could be determined")]
    NoBaseDirectory,
}

/// Resolves and creates the TokenMon data directory under an injectable base.
///
/// In production the base is `%LOCALAPPDATA%` (so data lives in
/// `%LOCALAPPDATA%\TokenMon`); tests pass a temp directory.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::{AppPaths, NoopPermissions};
/// let base = std::env::temp_dir().join(format!("tm-paths-doc-{}", std::process::id()));
/// let paths = AppPaths::with_permissions(&base, NoopPermissions);
/// let dir = paths.directory().unwrap();
/// assert_eq!(dir, base.join("TokenMon"));
/// assert!(dir.is_dir());
/// std::fs::remove_dir_all(&base).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct AppPaths {
    base: PathBuf,
    permissions: Arc<dyn OwnerOnlyPermissions>,
}

impl AppPaths {
    /// Paths under `base` with platform owner-only permissions.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self::with_permissions(base, PlatformPermissions)
    }

    /// Paths under `base` with a custom permission hook.
    pub fn with_permissions(
        base: impl Into<PathBuf>,
        permissions: impl OwnerOnlyPermissions + 'static,
    ) -> Self {
        Self {
            base: base.into(),
            permissions: Arc::new(permissions),
        }
    }

    /// Production paths from the process environment.
    pub fn production() -> Result<Self, PathsError> {
        Self::from_env(|name| std::env::var_os(name))
    }

    /// Resolves the base from an environment lookup: `LOCALAPPDATA`, else
    /// `XDG_DATA_HOME`, else `$HOME/.local/share` (non-Windows development).
    pub fn from_env(get: impl Fn(&str) -> Option<OsString>) -> Result<Self, PathsError> {
        let non_empty = |name: &str| get(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let base = non_empty("LOCALAPPDATA")
            .or_else(|| non_empty("XDG_DATA_HOME"))
            .or_else(|| non_empty("HOME").map(|h| h.join(".local").join("share")))
            .ok_or(PathsError::NoBaseDirectory)?;
        Ok(Self::new(base))
    }

    /// The base (parent of the `TokenMon` folder).
    pub fn base_directory(&self) -> &Path {
        &self.base
    }

    /// The shared permission hook (hand it to stores created by hand).
    pub fn permissions(&self) -> Arc<dyn OwnerOnlyPermissions> {
        Arc::clone(&self.permissions)
    }

    /// The `TokenMon` data directory: migrates a legacy `ModelMonitor` folder on
    /// first use, creates the directory and restricts it to the current user.
    pub fn directory(&self) -> io::Result<PathBuf> {
        let current = self.base.join(DIRECTORY_NAME);
        let legacy = self.base.join(LEGACY_DIRECTORY_NAME);
        if let Err(e) = migrate_legacy_directory_if_needed(&legacy, &current) {
            tracing::warn!(error = %e, "legacy directory migration failed");
        }
        self.create_restricted(&current)?;
        Ok(current)
    }

    /// A named subdirectory of the base (not of `TokenMon`), created and restricted.
    ///
    /// # Errors
    /// `InvalidInput` when `name` is empty or contains a path separator or `..`.
    pub fn subdirectory(&self, name: &str) -> io::Result<PathBuf> {
        if name == DIRECTORY_NAME {
            return self.directory();
        }
        let bad = name.is_empty() || name.contains(['/', '\\', '\0']) || name.contains("..");
        if bad {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid subdirectory name",
            ));
        }
        let path = self.base.join(name);
        self.create_restricted(&path)?;
        Ok(path)
    }

    /// A [`FileBackedStringStore`] with `prefix` in the data directory.
    pub fn string_store(&self, prefix: &str) -> io::Result<FileBackedStringStore> {
        Ok(FileBackedStringStore::new(self.directory()?, prefix)
            .with_shared_permissions(self.permissions()))
    }

    fn create_restricted(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)?;
        if let Err(e) = self.permissions.restrict_directory(path) {
            tracing::warn!(error = %e, "could not restrict directory to the current user");
        }
        Ok(())
    }
}

/// Moves `legacy` to `current` only when `legacy` exists and `current` does
/// not. Never merges or overwrites. Returns whether a move happened.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::migrate_legacy_directory_if_needed;
/// let base = std::env::temp_dir().join(format!("tm-mig-doc-{}", std::process::id()));
/// std::fs::create_dir_all(base.join("old")).unwrap();
/// assert!(migrate_legacy_directory_if_needed(&base.join("old"), &base.join("new")).unwrap());
/// assert!(base.join("new").is_dir() && !base.join("old").exists());
/// std::fs::remove_dir_all(&base).unwrap();
/// ```
pub fn migrate_legacy_directory_if_needed(legacy: &Path, current: &Path) -> io::Result<bool> {
    if legacy.is_dir() && !current.exists() {
        std::fs::rename(legacy, current)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// The user's real home directory from `USERPROFILE` (Windows) or `HOME`.
///
/// Used by the OpenCode local-database reader.
pub fn home_directory_from_env(get: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    ["USERPROFILE", "HOME"]
        .into_iter()
        .find_map(|n| get(n).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}
