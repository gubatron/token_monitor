//! The encrypted vault file shared by every provider (`KeychainVault`, spec
//! 1.6.3, with DPAPI in place of the Keychain item).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use super::encryptor::Encryptor;
use super::files::atomic_write;
use super::payload::{
    MAX_PAYLOAD_BYTES, VaultPayload, decode_payload, decode_pending, encode_payload,
};

/// Vault file name inside the TokenMon directory.
pub const VAULT_FILE_NAME: &str = "vault.bin";
/// Pending-removals record (plain JSON array of account names, no secrets).
pub const PENDING_FILE_NAME: &str = "vault_pending_removals.json";

/// DPAPI adds a small header; anything bigger than this is not our file.
const MAX_BLOB_BYTES: u64 = (MAX_PAYLOAD_BYTES as u64) * 2;

enum State {
    Unloaded,
    Loaded(VaultPayload),
    /// Unreadable or undecodable: every read is `None` and every write is
    /// refused for the rest of the process, so secrets we could not read are
    /// never overwritten.
    Unavailable,
}

struct Inner {
    state: State,
    pending: BTreeSet<String>,
}

/// One DPAPI-encrypted file holding every provider's secrets.
///
/// The file is read once and cached. Accounts are `<prefix><key>` strings
/// such as `claude_auth_session`. All access is serialized by one lock.
pub struct Vault {
    file_path: PathBuf,
    pending_path: PathBuf,
    encryptor: Arc<dyn Encryptor>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("file_path", &self.file_path)
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// A vault at `<dir>/vault.bin`. Nothing is read until first use.
    pub fn new(dir: impl Into<PathBuf>, encryptor: Arc<dyn Encryptor>) -> Self {
        let dir = dir.into();
        Self {
            file_path: dir.join(VAULT_FILE_NAME),
            pending_path: dir.join(PENDING_FILE_NAME),
            encryptor,
            inner: Mutex::new(Inner {
                state: State::Unloaded,
                pending: BTreeSet::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves no torn state we rely on.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The secret stored under `account`; `None` when missing or the vault is
    /// unavailable.
    pub fn value(&self, account: &str) -> Option<String> {
        let mut inner = self.lock();
        self.load_if_needed(&mut inner);
        match &inner.state {
            State::Loaded(p) => p.entries.get(account).map(|v| String::clone(v)),
            _ => None,
        }
    }

    /// Stores `value` under `account`, verified by reading the file back.
    /// False when unavailable or the write or verification failed (the cache
    /// is then unchanged).
    pub fn set(&self, value: &str, account: &str) -> bool {
        let mut inner = self.lock();
        self.load_if_needed(&mut inner);
        let State::Loaded(current) = &inner.state else {
            return false;
        };
        let mut next = current.clone();
        next.entries.insert(
            account.to_owned(),
            zeroize::Zeroizing::new(value.to_owned()),
        );
        if self.write(&next) {
            inner.state = State::Loaded(next);
            true
        } else {
            false
        }
    }

    /// Removes `account`. When the vault cannot be loaded, or the rewrite
    /// fails, the removal is recorded and applied on the next successful load.
    pub fn remove(&self, account: &str) {
        let mut inner = self.lock();
        self.load_if_needed(&mut inner);
        let State::Loaded(current) = &inner.state else {
            inner.pending.insert(account.to_owned());
            self.persist_pending(&inner.pending);
            return;
        };
        if !current.entries.contains_key(account) {
            return;
        }
        let mut next = current.clone();
        next.entries.remove(account);
        if self.write(&next) {
            inner.state = State::Loaded(next);
        } else {
            inner.pending.insert(account.to_owned());
            self.persist_pending(&inner.pending);
        }
    }

    /// Whether the vault is locked out for this process (test and UI hook).
    pub fn is_unavailable(&self) -> bool {
        let mut inner = self.lock();
        self.load_if_needed(&mut inner);
        matches!(inner.state, State::Unavailable)
    }

    fn load_if_needed(&self, inner: &mut Inner) {
        if !matches!(inner.state, State::Unloaded) {
            return;
        }
        inner.state = self.read_vault();
        if matches!(inner.state, State::Loaded(_)) {
            self.apply_pending(inner);
        }
    }

    fn read_vault(&self) -> State {
        match self.read_payload() {
            Ok(Some(p)) => State::Loaded(p),
            Ok(None) => State::Loaded(VaultPayload::default()),
            Err(()) => State::Unavailable,
        }
    }

    /// `Ok(None)` when the file does not exist yet.
    fn read_payload(&self) -> Result<Option<VaultPayload>, ()> {
        let blob = match read_bounded(&self.file_path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                tracing::warn!(error = %e, "vault unreadable, locking it for this run");
                return Err(());
            }
        };
        let plain = self.encryptor.unprotect(&blob).map_err(|e| {
            tracing::warn!(error = %e, "vault decryption failed, locking it for this run");
        })?;
        decode_payload(&plain).map(Some).map_err(|e| {
            tracing::warn!(error = %e, "vault does not decode, locking it for this run");
        })
    }

    /// Encrypt, atomically replace the file, read it back and compare.
    fn write(&self, payload: &VaultPayload) -> bool {
        let plain = encode_payload(payload);
        let blob = match self.encryptor.protect(&plain) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "vault encryption failed");
                return false;
            }
        };
        if let Err(e) = atomic_write(&self.file_path, &blob) {
            tracing::warn!(error = %e, "vault write failed");
            return false;
        }
        match self.read_payload() {
            Ok(Some(back)) if back == *payload => true,
            _ => {
                tracing::warn!("vault read-back mismatch");
                false
            }
        }
    }

    fn apply_pending(&self, inner: &mut Inner) {
        inner.pending.extend(self.load_pending_file());
        if inner.pending.is_empty() {
            return;
        }
        let State::Loaded(current) = &inner.state else {
            return;
        };
        let mut next = current.clone();
        let mut changed = false;
        for account in &inner.pending {
            changed |= next.entries.remove(account).is_some();
        }
        let done = if changed {
            if self.write(&next) {
                inner.state = State::Loaded(next);
                true
            } else {
                false
            }
        } else {
            true
        };
        if done {
            inner.pending.clear();
            self.persist_pending(&inner.pending);
        }
    }

    fn load_pending_file(&self) -> BTreeSet<String> {
        match read_bounded(&self.pending_path) {
            Ok(b) => decode_pending(&b).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "pending-removals record ignored");
                BTreeSet::new()
            }),
            Err(_) => BTreeSet::new(),
        }
    }

    /// Best effort: the in-memory set still applies for this process.
    fn persist_pending(&self, pending: &BTreeSet<String>) {
        if pending.is_empty() {
            let _ = fs::remove_file(&self.pending_path);
            return;
        }
        let Ok(json) = serde_json::to_vec(pending) else {
            return;
        };
        if let Err(e) = atomic_write(&self.pending_path, &json) {
            tracing::warn!(error = %e, "could not persist pending removals");
        }
    }
}

/// Reads a file, refusing oversized ones before allocating.
fn read_bounded(path: &std::path::Path) -> io::Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_BLOB_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
    }
    fs::read(path)
}
