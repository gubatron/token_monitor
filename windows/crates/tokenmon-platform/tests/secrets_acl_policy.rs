#![allow(clippy::unwrap_used)]
//! Owner-only creation of secrets files and the plaintext-fallback policy.

use std::fs;
use std::sync::Arc;

use tempfile::TempDir;
use tokenmon_core::{CredentialKey, CredentialStore};
use tokenmon_platform::secrets::{
    APP_FALLBACK_POLICY, FakeEncryptor, FallbackPolicy, SecretRoutingCredentialStore,
    SecretStoreError, VAULT_UNAVAILABLE_REASON, Vault, create_new_owner_only, owner_only_sddl,
};

#[test]
fn sddl_is_a_protected_single_ace_dacl() {
    let sddl = owner_only_sddl("S-1-5-21-1-2-3-1001");
    assert_eq!(sddl, "D:P(A;;FA;;;S-1-5-21-1-2-3-1001)");
    assert!(sddl.starts_with("D:P"), "inheritance must be blocked");
    assert_eq!(sddl.matches("(A;").count(), 1, "exactly one ACE");
}

#[test]
fn create_new_refuses_an_existing_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("x.dat");
    fs::write(&path, b"pre-created by someone else").unwrap();
    assert!(create_new_owner_only(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"pre-created by someone else");
}

#[cfg(unix)]
#[test]
fn every_secrets_file_is_created_0600() {
    use std::os::unix::fs::PermissionsExt;
    use tokenmon_platform::secrets::{VAULT_FILE_NAME, atomic_write};
    let dir = TempDir::new().unwrap();
    let vault = Arc::new(Vault::new(dir.path(), Arc::new(FakeEncryptor::default())));
    assert!(vault.set("secret", "a_session"));
    atomic_write(&dir.path().join("a_email.dat"), b"me@x").unwrap();
    for name in [VAULT_FILE_NAME, "a_email.dat"] {
        let mode = fs::metadata(dir.path().join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name}");
    }
}

#[test]
fn the_app_default_policy_is_the_spec_plain_file_fallback() {
    assert_eq!(APP_FALLBACK_POLICY, FallbackPolicy::PlainFile);
}

fn broken_vault_store(
    dir: &TempDir,
    policy: FallbackPolicy,
) -> (SecretRoutingCredentialStore, Arc<FakeEncryptor>) {
    let enc = Arc::new(FakeEncryptor::default());
    let vault = Arc::new(Vault::new(dir.path(), enc.clone()));
    enc.set_fail_protect(true);
    (
        SecretRoutingCredentialStore::new(vault, dir.path(), "a_", policy),
        enc,
    )
}

#[test]
fn refuse_policy_surfaces_a_typed_vault_unavailable_error() {
    let dir = TempDir::new().unwrap();
    let (store, _) = broken_vault_store(&dir, FallbackPolicy::Refuse);
    let err = store.set(CredentialKey::Session, "cookie").unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("credential store write failed: {VAULT_UNAVAILABLE_REASON}")
    );
    let ui = SecretStoreError::classify(&err);
    assert_eq!(ui, SecretStoreError::VaultUnavailable);
    assert!(ui.to_string().contains("vault is unavailable"));
    assert!(!dir.path().join("a_session.dat").exists(), "no plaintext");
    assert_eq!(store.get(CredentialKey::Session), None);
}

#[test]
fn plain_file_policy_still_stores_and_reports_success() {
    let dir = TempDir::new().unwrap();
    let (store, _) = broken_vault_store(&dir, FallbackPolicy::PlainFile);
    store.set(CredentialKey::Session, "cookie").unwrap();
    assert_eq!(store.get(CredentialKey::Session).as_deref(), Some("cookie"));
    assert!(dir.path().join("a_session.dat").exists());
}

#[test]
fn other_write_failures_are_classified_as_other() {
    let err = tokenmon_core::CredentialStoreError::WriteFailed("disk full".into());
    assert_eq!(
        SecretStoreError::classify(&err),
        SecretStoreError::Other("disk full".into())
    );
}

#[cfg(windows)]
#[test]
fn windows_file_gets_only_the_current_user_ace_at_creation() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("vault.bin");
    drop(create_new_owner_only(&path).unwrap());
    let out = std::process::Command::new("icacls")
        .arg(&path)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for wide in ["Everyone", "BUILTIN\\Users", "Authenticated Users"] {
        assert!(!text.contains(wide), "{wide} has access:\n{text}");
    }
    // Protected: no inherited ACE "(I)" survives.
    assert!(!text.contains("(I)"), "inherited ACE present:\n{text}");
}
