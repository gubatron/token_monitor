//! Per-key atomic text files and an in-memory fake.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::atomic::write_atomic;
use super::permissions::{OwnerOnlyPermissions, PlatformPermissions};

/// A keyed text store: the seam delta stores and credential stores code against.
///
/// Failures are reported as `None`/`false` (and logged), never panics; callers
/// treat storage as best-effort.
pub trait TextStore: Send + Sync {
    /// The stored text, or `None` when missing or unreadable.
    fn value(&self, key: &str) -> Option<String>;

    /// Stores `value` atomically. Returns `false` on failure.
    fn set(&self, value: &str, key: &str) -> bool;

    /// Deletes the value. A missing value is fine.
    fn remove(&self, key: &str);
}

/// Atomic per-key text files: `<directory>/<prefix><key>.dat`.
///
/// Writes go through [`write_atomic`], so the directory only ever contains the
/// target files (no stray staging files) and files are owner-only.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::{FileBackedStringStore, NoopPermissions, TextStore};
/// let dir = std::env::temp_dir().join(format!("tm-store-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&dir).unwrap();
/// let store = FileBackedStringStore::new(dir.clone(), "auth_").with_permissions(NoopPermissions);
/// assert!(store.set("a@b.c", "email"));
/// assert_eq!(store.value("email").as_deref(), Some("a@b.c"));
/// std::fs::remove_dir_all(&dir).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct FileBackedStringStore {
    directory: PathBuf,
    prefix: String,
    permissions: Arc<dyn OwnerOnlyPermissions>,
}

impl FileBackedStringStore {
    /// The default filename prefix used by the Grok credential store.
    pub const DEFAULT_PREFIX: &'static str = "auth_";

    /// A store in `directory` with `prefix` and platform owner-only permissions.
    pub fn new(directory: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        Self {
            directory: directory.into(),
            prefix: prefix.into(),
            permissions: Arc::new(PlatformPermissions),
        }
    }

    /// Replaces the permission hook (tests, or a shared instance from `AppPaths`).
    #[must_use]
    pub fn with_permissions(self, permissions: impl OwnerOnlyPermissions + 'static) -> Self {
        Self {
            permissions: Arc::new(permissions),
            ..self
        }
    }

    /// Replaces the permission hook with an already shared one.
    #[must_use]
    pub fn with_shared_permissions(self, permissions: Arc<dyn OwnerOnlyPermissions>) -> Self {
        Self {
            permissions,
            ..self
        }
    }

    /// The directory holding the files.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The path of the file for `key`, or `None` if `key` could escape the directory.
    pub fn path_for(&self, key: &str) -> Option<PathBuf> {
        let unsafe_key = key.is_empty()
            || key.contains(['/', '\\', '\0', ':'])
            || key == ".."
            || key.contains("..");
        (!unsafe_key).then(|| self.directory.join(format!("{}{key}.dat", self.prefix)))
    }
}

impl TextStore for FileBackedStringStore {
    fn value(&self, key: &str) -> Option<String> {
        let path = self.path_for(key)?;
        match std::fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(key, error = %e, "file store read failed");
                None
            }
        }
    }

    fn set(&self, value: &str, key: &str) -> bool {
        let Some(path) = self.path_for(key) else {
            tracing::warn!(key, "file store rejected unsafe key");
            return false;
        };
        match write_atomic(&path, value.as_bytes(), self.permissions.as_ref()) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(key, error = %e, "file store write failed");
                false
            }
        }
    }

    fn remove(&self, key: &str) {
        let Some(path) = self.path_for(key) else {
            return;
        };
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(key, error = %e, "file store remove failed"),
        }
    }
}

/// A [`TextStore`] kept in memory, with a write counter. For hermetic tests.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::{InMemoryStringStore, TextStore};
/// let store = InMemoryStringStore::default();
/// assert!(store.set("v", "k"));
/// assert_eq!(store.write_count(), 1);
/// ```
#[derive(Debug, Default)]
pub struct InMemoryStringStore {
    values: Mutex<BTreeMap<String, String>>,
    writes: AtomicUsize,
}

impl InMemoryStringStore {
    /// How many successful `set` calls were made.
    pub fn write_count(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
}

impl TextStore for InMemoryStringStore {
    fn value(&self, key: &str) -> Option<String> {
        self.values.lock().ok()?.get(key).cloned()
    }

    fn set(&self, value: &str, key: &str) -> bool {
        let Ok(mut guard) = self.values.lock() else {
            return false;
        };
        guard.insert(key.to_owned(), value.to_owned());
        self.writes.fetch_add(1, Ordering::SeqCst);
        true
    }

    fn remove(&self, key: &str) {
        if let Ok(mut guard) = self.values.lock() {
            guard.remove(key);
        }
    }
}
