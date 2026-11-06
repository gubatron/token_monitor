//! Registry access behind a trait, with an in-memory fake.
//!
//! All keys are relative to `HKEY_CURRENT_USER`; the app never writes to
//! `HKLM`, so no elevation is ever needed. An empty value name addresses the
//! key's default value, as in the Windows API.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use crate::shell::error::ShellError;

/// Minimal HKCU registry surface used by the shell integrations.
///
/// Deleting something that does not exist is **not** an error: every delete is
/// idempotent so callers can "make sure it is gone" without a prior read.
pub trait Registry: Send + Sync {
    /// Reads a `REG_SZ` value; `Ok(None)` when the key or value is missing
    /// (or holds a different type).
    fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, ShellError>;
    /// Creates the key if needed and writes a `REG_SZ` value.
    fn set_string(&self, key: &str, name: &str, value: &str) -> Result<(), ShellError>;
    /// Reads a `REG_BINARY` value; `Ok(None)` when missing.
    fn get_binary(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, ShellError>;
    /// Deletes one value; succeeds when it is already gone.
    fn delete_value(&self, key: &str, name: &str) -> Result<(), ShellError>;
    /// Deletes a key with all subkeys and values; succeeds when it is already gone.
    fn delete_key_tree(&self, key: &str) -> Result<(), ShellError>;
    /// Whether the key exists.
    fn key_exists(&self, key: &str) -> Result<bool, ShellError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Default)]
struct State {
    keys: BTreeSet<String>,
    values: BTreeMap<(String, String), Value>,
    fail_writes: bool,
}

/// In-memory [`Registry`] for tests. Key and value names are case-insensitive,
/// like the real registry.
#[derive(Debug, Default)]
pub struct MemoryRegistry {
    state: Mutex<State>,
}

fn norm(s: &str) -> String {
    s.replace('/', "\\").trim_matches('\\').to_lowercase()
}

fn add_key_chain(keys: &mut BTreeSet<String>, key: &str) {
    let mut acc = String::new();
    for part in key.split('\\').filter(|p| !p.is_empty()) {
        if !acc.is_empty() {
            acc.push('\\');
        }
        acc.push_str(part);
        keys.insert(acc.clone());
    }
}

impl MemoryRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every subsequent write fail, to test error paths.
    pub fn set_fail_writes(&self, fail: bool) {
        self.lock().fail_writes = fail;
    }

    /// Seeds a `REG_BINARY` value (e.g. `StartupApproved`), creating the key.
    pub fn seed_binary(&self, key: &str, name: &str, data: &[u8]) {
        let mut s = self.lock();
        add_key_chain(&mut s.keys, &norm(key));
        s.values
            .insert((norm(key), norm(name)), Value::Binary(data.to_vec()));
    }

    /// Every key currently present, normalised (lower case), sorted.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.lock().keys.iter().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_guard(&self) -> Result<std::sync::MutexGuard<'_, State>, ShellError> {
        let s = self.lock();
        if s.fail_writes {
            Err(ShellError::Registry("injected failure".into()))
        } else {
            Ok(s)
        }
    }
}

impl Registry for MemoryRegistry {
    fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, ShellError> {
        Ok(match self.lock().values.get(&(norm(key), norm(name))) {
            Some(Value::Text(t)) => Some(t.clone()),
            _ => None,
        })
    }

    fn set_string(&self, key: &str, name: &str, value: &str) -> Result<(), ShellError> {
        let mut s = self.write_guard()?;
        add_key_chain(&mut s.keys, &norm(key));
        s.values
            .insert((norm(key), norm(name)), Value::Text(value.to_owned()));
        Ok(())
    }

    fn get_binary(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, ShellError> {
        Ok(match self.lock().values.get(&(norm(key), norm(name))) {
            Some(Value::Binary(b)) => Some(b.clone()),
            _ => None,
        })
    }

    fn delete_value(&self, key: &str, name: &str) -> Result<(), ShellError> {
        self.write_guard()?.values.remove(&(norm(key), norm(name)));
        Ok(())
    }

    fn delete_key_tree(&self, key: &str) -> Result<(), ShellError> {
        let mut s = self.write_guard()?;
        let root = norm(key);
        let prefix = format!("{root}\\");
        let inside = |k: &str| k == root || k.starts_with(&prefix);
        s.keys.retain(|k| !inside(k));
        s.values.retain(|(k, _), _| !inside(k));
        Ok(())
    }

    fn key_exists(&self, key: &str) -> Result<bool, ShellError> {
        Ok(self.lock().keys.contains(&norm(key)))
    }
}
