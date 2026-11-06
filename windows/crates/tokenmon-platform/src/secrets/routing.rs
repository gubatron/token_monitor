//! `SecretRoutingCredentialStore`: secrets to the vault, identity values to
//! plain files (spec 1.6.2).

use std::path::PathBuf;
use std::sync::Arc;

use tokenmon_core::{CredentialKey, CredentialKind, CredentialStore, CredentialStoreError};

use super::files::FileBackedCredentialStore;
use super::vault::Vault;

/// Every key, in `Ord` order (the migration order).
const ALL_KEYS: [CredentialKey; 5] = [
    CredentialKey::Session,
    CredentialKey::Email,
    CredentialKey::Account,
    CredentialKey::Workspace,
    CredentialKey::ApiKey,
];

/// What `set` does for a secret when the vault refuses the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FallbackPolicy {
    /// Spec 1.6.2: keep the app working by writing the secret to its plain
    /// owner-only file; it migrates into the vault on the next start.
    #[default]
    PlainFile,
    /// Fail the write instead; the secret never touches disk unencrypted.
    Refuse,
}

/// Production [`CredentialStore`] for one provider prefix.
#[derive(Debug)]
pub struct SecretRoutingCredentialStore {
    vault: Arc<Vault>,
    files: FileBackedCredentialStore,
    prefix: String,
    policy: FallbackPolicy,
}

impl SecretRoutingCredentialStore {
    /// Builds the store and migrates any secret still sitting in a plain file
    /// into the vault (the file is deleted only after a verified read-back).
    pub fn new(
        vault: Arc<Vault>,
        dir: impl Into<PathBuf>,
        prefix: impl Into<String>,
        policy: FallbackPolicy,
    ) -> Self {
        let prefix = prefix.into();
        let store = Self {
            vault,
            files: FileBackedCredentialStore::new(dir, prefix.clone()),
            prefix,
            policy,
        };
        store.migrate();
        store
    }

    fn account(&self, key: CredentialKey) -> String {
        format!("{}{}", self.prefix, key.as_str())
    }

    fn migrate(&self) {
        for key in ALL_KEYS
            .into_iter()
            .filter(|k| k.kind() == CredentialKind::Secret)
        {
            let Some(value) = self.files.get(key) else {
                continue;
            };
            let account = self.account(key);
            if self.vault.set(&value, &account)
                && self.vault.value(&account).as_deref() == Some(value.as_str())
            {
                self.files.remove(key);
            } else {
                tracing::warn!(key = key.as_str(), "vault migration failed, file kept");
            }
        }
    }
}

impl CredentialStore for SecretRoutingCredentialStore {
    fn get(&self, key: CredentialKey) -> Option<String> {
        match key.kind() {
            CredentialKind::Plain => self.files.get(key),
            // A file wins: it exists only if the vault migration failed.
            CredentialKind::Secret => self
                .files
                .get(key)
                .or_else(|| self.vault.value(&self.account(key))),
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
        match self.policy {
            FallbackPolicy::PlainFile => {
                tracing::warn!(key = key.as_str(), "vault write failed, using plain file");
                self.files.set(key, value)
            }
            FallbackPolicy::Refuse => Err(CredentialStoreError::WriteFailed(
                "vault unavailable".into(),
            )),
        }
    }

    fn remove(&self, key: CredentialKey) {
        if key.kind() == CredentialKind::Secret {
            self.vault.remove(&self.account(key));
        }
        self.files.remove(key);
    }
}
