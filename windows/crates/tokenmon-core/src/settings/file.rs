//! The on-disk JSON object that backs [`AppSettings`](super::AppSettings) and the
//! threshold-notifier records.
//!
//! This is the Windows stand-in for `UserDefaults.standard` (spec 01 section 8):
//! one flat JSON object whose key names and value types match spec 01 section 2.
//! Reading is total: a missing, unreadable, truncated or non-object file yields an
//! empty map, so the app always starts with defaults instead of failing.
//!
//! `ws-core-storage` owns the shared `AppPaths` and atomic-write helpers; until
//! they land this module keeps a small private atomic writer. Callers pass the
//! settings file path explicitly, so swapping the path source is a one-line change.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value};

/// A JSON object keyed by setting name.
pub type SettingsMap = Map<String, Value>;

/// Decodes `bytes` into a settings map; anything that is not a JSON object
/// (corrupt, truncated, an array, empty) becomes an empty map.
///
/// # Examples
///
/// ```
/// use tokenmon_core::settings::decode_map;
///
/// assert_eq!(decode_map(br#"{"activePollSeconds": 30}"#).len(), 1);
/// assert!(decode_map(b"\xff\x00 not json").is_empty());
/// assert!(decode_map(b"[1,2]").is_empty());
/// ```
pub fn decode_map(bytes: &[u8]) -> SettingsMap {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => map,
        _ => SettingsMap::new(),
    }
}

/// A thread-safe, optionally file-backed settings object.
///
/// Every mutation is written through to disk atomically (temp file plus rename,
/// owner-only permissions on Unix). A failed write is logged and the in-memory
/// value is kept, mirroring how `UserDefaults` never throws.
#[derive(Debug)]
pub struct SettingsFile {
    path: Option<PathBuf>,
    map: Mutex<SettingsMap>,
}

impl SettingsFile {
    /// Opens `path`, treating a missing or corrupt file as empty.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let map = match std::fs::read(&path) {
            Ok(bytes) => decode_map(&bytes),
            Err(_) => SettingsMap::new(),
        };
        Self {
            path: Some(path),
            map: Mutex::new(map),
        }
    }

    /// A store that never touches the disk (tests, previews).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            map: Mutex::new(SettingsMap::new()),
        }
    }

    /// Builds an in-memory store seeded from raw file bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            path: None,
            map: Mutex::new(decode_map(bytes)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SettingsMap> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The backing file path, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Returns a copy of the value stored under `key`.
    pub fn get(&self, key: &str) -> Option<Value> {
        self.lock().get(key).cloned()
    }

    /// Returns a copy of the whole map.
    pub fn snapshot(&self) -> SettingsMap {
        self.lock().clone()
    }

    /// Applies `edits` (`None` removes the key) and writes the file once.
    pub fn apply<I>(&self, edits: I)
    where
        I: IntoIterator<Item = (String, Option<Value>)>,
    {
        let mut guard = self.lock();
        for (key, value) in edits {
            match value {
                Some(v) => {
                    guard.insert(key, v);
                }
                None => {
                    guard.remove(&key);
                }
            }
        }
        if let Some(path) = &self.path
            && let Err(err) = write_atomic(path, &guard)
        {
            tracing::warn!(path = %path.display(), error = %err, "could not persist settings");
        }
    }

    /// Sets one key.
    pub fn set(&self, key: &str, value: Value) {
        self.apply([(key.to_owned(), Some(value))]);
    }

    /// Removes one key.
    pub fn remove(&self, key: &str) {
        self.apply([(key.to_owned(), None)]);
    }
}

fn write_atomic(path: &Path, map: &SettingsMap) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(map).map_err(std::io::Error::other)?;
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
