//! `SecretRoutingCredentialStore`: secrets to the vault, identity values to
//! plain files (spec 1.6.2, as amended by decision #38: a secret never touches
//! a plain file).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokenmon_core::{CredentialKey, CredentialKind, CredentialStore, CredentialStoreError};

use super::files::{FileBackedCredentialStore, secure_delete};
use super::vault::Vault;

/// Every key, in `Ord` order (the migration order).
const ALL_KEYS: [CredentialKey; 5] = [
    CredentialKey::Session,
    CredentialKey::Email,
    CredentialKey::Account,
    CredentialKey::Workspace,
    CredentialKey::ApiKey,
];

/// The `WriteFailed` reason the store reports when the vault refuses a secret
/// (a secret is never written to a plain file, maintainer decision #38). Classify it with [`SecretStoreError::classify`].
pub const VAULT_UNAVAILABLE_REASON: &str = "vault unavailable";

/// A credential write failure, classified for the sign-in UI.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretStoreError {
    /// The encrypted vault could not take the secret and a plaintext file is
    /// never an option, so nothing was stored and the user is not signed in.
    #[error(
        "Could not save your sign-in securely: the encrypted credential vault is unavailable. \
         Nothing was stored. Restart TokenMon or sign in again."
    )]
    VaultUnavailable,
    /// Any other write failure (disk full, access denied, ...).
    #[error("Could not save your sign-in: {0}")]
    Other(String),
}

impl SecretStoreError {
    /// Classifies a store error for display.
    pub fn classify(error: &CredentialStoreError) -> Self {
        match error {
            CredentialStoreError::WriteFailed(reason) if reason == VAULT_UNAVAILABLE_REASON => {
                Self::VaultUnavailable
            }
            CredentialStoreError::WriteFailed(reason) => Self::Other(reason.clone()),
        }
    }
}

/// Production [`CredentialStore`] for one provider prefix.
#[derive(Debug)]
pub struct SecretRoutingCredentialStore {
    vault: Arc<Vault>,
    files: FileBackedCredentialStore,
    prefix: String,
}

impl SecretRoutingCredentialStore {
    /// Builds the store and migrates any secret still sitting in a plain file
    /// into the vault (see `migrate`: the file is always deleted).
    pub fn new(vault: Arc<Vault>, dir: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        let store = Self {
            vault,
            files: FileBackedCredentialStore::new(dir, prefix.clone()),
            prefix,
        };
        store.migrate();
        store
    }

    fn account(&self, key: CredentialKey) -> String {
        format!("{}{}", self.prefix, key.as_str())
    }

    /// A secret in a plain `.dat` file (an older build, or the retired
    /// fallback) is moved into the vault and the file securely deleted. When
    /// the vault refuses it the file is deleted anyway and the account is left
    /// signed out: a credential is never kept unencrypted (decision #38).
    /// Nothing here logs a value.
    fn migrate(&self) {
        for key in ALL_KEYS
            .into_iter()
            .filter(|k| k.kind() == CredentialKind::Secret)
        {
            self.sweep_staging_files(key);
            let path = self.files.path(key);
            if std::fs::symlink_metadata(&path).is_err() {
                continue;
            }
            let account = self.account(key);
            let moved = self.files.get(key).is_some_and(|value| {
                let value = zeroize::Zeroizing::new(value);
                !value.is_empty()
                    && self.vault.set(&value, &account)
                    && self.vault.value(&account).as_deref() == Some(value.as_str())
            });
            if moved {
                tracing::info!(
                    key = key.as_str(),
                    "moved a plaintext credential into the vault"
                );
            } else {
                // Drop any older vault copy too, so the provider is signed out.
                self.vault.remove(&account);
                tracing::warn!(
                    key = key.as_str(),
                    "could not move a plaintext credential into the vault; deleted it, sign in again"
                );
            }
            if let Err(e) = secure_delete(&path) {
                tracing::error!(key = key.as_str(), error = %e, "could not delete a plaintext credential file");
            }
        }
    }

    /// Leftover `.<name>.<pid>-<n>.tmp` staging files of a crashed write.
    fn sweep_staging_files(&self, key: CredentialKey) {
        let path = self.files.path(key);
        let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
        else {
            return;
        };
        let stem = format!(".{name}.");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let file = entry.file_name();
            let Some(file) = file.to_str() else { continue };
            if file.starts_with(&stem)
                && Path::new(file)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("tmp"))
            {
                let _ = secure_delete(&entry.path());
            }
        }
    }
}

impl CredentialStore for SecretRoutingCredentialStore {
    fn get(&self, key: CredentialKey) -> Option<String> {
        match key.kind() {
            CredentialKind::Plain => self.files.get(key),
            CredentialKind::Secret => self.vault.value(&self.account(key)),
        }
    }

    fn set(&self, key: CredentialKey, value: &str) -> Result<(), CredentialStoreError> {
        if key.kind() == CredentialKind::Plain {
            return self.files.set(key, value);
        }
        if self.vault.set(value, &self.account(key)) {
            self.files.remove(key);
            return Ok(());
        }
        Err(CredentialStoreError::WriteFailed(
            VAULT_UNAVAILABLE_REASON.into(),
        ))
    }

    fn remove(&self, key: CredentialKey) {
        if key.kind() == CredentialKind::Secret {
            self.vault.remove(&self.account(key));
        }
        self.files.remove(key);
    }
}
