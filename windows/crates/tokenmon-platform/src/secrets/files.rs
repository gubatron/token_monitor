//! Plain owner-only credential files and the atomic-write helper.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokenmon_core::{CredentialKey, CredentialStore, CredentialStoreError};

use super::secure_file::create_new_owner_only;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to `path` atomically: a staging file in the same directory
/// (created owner-only: mode 0600 on Unix, a protected current-user-only DACL
/// on Windows, applied at creation so there is no window with a wider ACL), synced,
/// then renamed over the target. On failure no staging file is left behind and
/// a missing directory is an error (it is never created).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad file name"))?;
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!(".{name}.{}-{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = create_new_owner_only(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// `FileBackedCredentialStore(prefix)`: `<dir>/<prefix><key>.dat`, atomic
/// writes, owner-only.
#[derive(Debug, Clone)]
pub struct FileBackedCredentialStore {
    dir: PathBuf,
    prefix: String,
}

impl FileBackedCredentialStore {
    /// A store of files named `<prefix><key>.dat` inside `dir`.
    pub fn new(dir: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            prefix: prefix.into(),
        }
    }

    /// The file backing `key`.
    pub fn path(&self, key: CredentialKey) -> PathBuf {
        self.dir
            .join(format!("{}{}.dat", self.prefix, key.as_str()))
    }
}

impl CredentialStore for FileBackedCredentialStore {
    fn get(&self, key: CredentialKey) -> Option<String> {
        match fs::read(self.path(key)) {
            Ok(b) => String::from_utf8(b).ok(),
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(key = key.as_str(), error = %e, "credential file read failed");
                }
                None
            }
        }
    }

    fn set(&self, key: CredentialKey, value: &str) -> Result<(), CredentialStoreError> {
        atomic_write(&self.path(key), value.as_bytes()).map_err(|e| {
            tracing::warn!(key = key.as_str(), error = %e, "credential file write failed");
            CredentialStoreError::WriteFailed(e.to_string())
        })
    }

    fn remove(&self, key: CredentialKey) {
        if let Err(e) = fs::remove_file(self.path(key))
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(key = key.as_str(), error = %e, "credential file remove failed");
        }
    }
}
