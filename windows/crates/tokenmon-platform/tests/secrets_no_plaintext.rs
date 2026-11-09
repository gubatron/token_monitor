#![allow(clippy::unwrap_used)]
//! Decision #38: a credential is never stored unencrypted. Plaintext secret
//! files from older builds are migrated into the vault or deleted, a failed
//! vault write fails the save, and no secret byte ever reaches the disk.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;
use tokenmon_core::{CredentialKey, CredentialStore};
use tokenmon_platform::secrets::{
    FakeEncryptor, FileBackedCredentialStore, SecretRoutingCredentialStore, SecretStoreError,
    VAULT_FILE_NAME, Vault, secure_delete,
};

const SECRET: &str = "PLAINTEXT-CANARY-0xC0FFEE-not-a-real-secret";
const NEEDLE: &[u8] = b"PLAINTEXT-CANARY-0xC0FFEE";

fn vault_in(dir: &Path, enc: &Arc<FakeEncryptor>) -> Arc<Vault> {
    Arc::new(Vault::new(dir, enc.clone()))
}

fn store(dir: &Path, vault: &Arc<Vault>, prefix: &str) -> SecretRoutingCredentialStore {
    SecretRoutingCredentialStore::new(vault.clone(), dir, prefix)
}

fn legacy(dir: &Path, prefix: &str) -> FileBackedCredentialStore {
    FileBackedCredentialStore::new(dir, prefix)
}

/// Every file under `dir`, recursively, that contains the secret marker.
fn files_containing_secret(dir: &Path) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if fs::read(&p)
                .unwrap()
                .windows(NEEDLE.len())
                .any(|w| w == NEEDLE)
            {
                hits.push(p.display().to_string());
            }
        }
    }
    hits
}

#[test]
fn migration_moves_the_secret_into_the_vault_and_deletes_the_file() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let plain = legacy(dir.path(), "claude_auth_");
    plain.set(CredentialKey::Session, SECRET).unwrap();
    plain.set(CredentialKey::Email, "a@b.com").unwrap();

    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");

    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some(SECRET));
    assert!(!plain.path(CredentialKey::Session).exists());
    assert_eq!(s.get(CredentialKey::Email).as_deref(), Some("a@b.com"));
    assert!(dir.path().join(VAULT_FILE_NAME).exists());
    assert_eq!(files_containing_secret(dir.path()), Vec::<String>::new());
}

#[test]
fn migration_covers_the_openrouter_api_key() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    legacy(dir.path(), "openrouter_auth_")
        .set(CredentialKey::ApiKey, SECRET)
        .unwrap();
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "openrouter_auth_");
    assert_eq!(s.get(CredentialKey::ApiKey).as_deref(), Some(SECRET));
    assert!(!dir.path().join("openrouter_auth_key.dat").exists());
}

#[test]
fn failed_migration_deletes_the_plaintext_and_signs_the_provider_out() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let plain = legacy(dir.path(), "claude_auth_");
    plain.set(CredentialKey::Session, SECRET).unwrap();
    plain.set(CredentialKey::Email, "a@b.com").unwrap();
    enc.set_fail_protect(true);

    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");

    assert!(!plain.path(CredentialKey::Session).exists());
    assert_eq!(s.get(CredentialKey::Session), None, "signed out");
    assert_eq!(s.get(CredentialKey::Email).as_deref(), Some("a@b.com"));
    assert_eq!(files_containing_secret(dir.path()), Vec::<String>::new());

    // Signing in again works once the vault does.
    enc.set_fail_protect(false);
    s.set(CredentialKey::Session, "fresh").unwrap();
    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some("fresh"));
}

#[test]
fn migration_with_a_locked_out_vault_still_deletes_the_plaintext() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    fs::write(dir.path().join(VAULT_FILE_NAME), b"not a vault").unwrap();
    legacy(dir.path(), "a_")
        .set(CredentialKey::Session, SECRET)
        .unwrap();

    let vault = vault_in(dir.path(), &enc);
    let s = store(dir.path(), &vault, "a_");

    assert!(vault.is_unavailable());
    assert_eq!(s.get(CredentialKey::Session), None);
    assert_eq!(files_containing_secret(dir.path()), Vec::<String>::new());
    assert_eq!(
        fs::read(dir.path().join(VAULT_FILE_NAME)).unwrap(),
        b"not a vault",
        "the unreadable vault is never overwritten"
    );
}

#[test]
fn migration_deletes_unreadable_and_empty_secret_files_and_staging_leftovers() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    fs::write(dir.path().join("a_session.dat"), [0xff, 0xfe, 0x00]).unwrap();
    fs::write(dir.path().join("b_session.dat"), b"").unwrap();
    fs::write(
        dir.path().join(".c_session.dat.99-0.tmp"),
        SECRET.as_bytes(),
    )
    .unwrap();
    let vault = vault_in(dir.path(), &enc);
    for prefix in ["a_", "b_", "c_"] {
        let s = store(dir.path(), &vault, prefix);
        assert_eq!(s.get(CredentialKey::Session), None, "{prefix}");
    }
    assert!(!dir.path().join("a_session.dat").exists());
    assert!(!dir.path().join("b_session.dat").exists());
    assert!(!dir.path().join(".c_session.dat.99-0.tmp").exists());
}

#[test]
fn a_stale_plaintext_file_replaces_an_older_vault_copy() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    assert!(vault_in(dir.path(), &enc).set("old", "a_session"));
    legacy(dir.path(), "a_")
        .set(CredentialKey::Session, "newer")
        .unwrap();
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "a_");
    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some("newer"));
}

#[test]
fn a_refused_save_fails_with_vault_unavailable_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");
    enc.set_fail_protect(true);

    let err = s.set(CredentialKey::Session, SECRET).unwrap_err();

    assert_eq!(
        SecretStoreError::classify(&err),
        SecretStoreError::VaultUnavailable
    );
    assert_eq!(s.get(CredentialKey::Session), None);
    assert!(!dir.path().join("claude_auth_session.dat").exists());
    assert_eq!(files_containing_secret(dir.path()), Vec::<String>::new());
}

#[test]
fn no_secret_byte_reaches_the_disk_through_any_operation() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");
    s.set(CredentialKey::Session, SECRET).unwrap();
    s.set(CredentialKey::Email, "a@b.com").unwrap();
    enc.set_fail_protect(true);
    assert!(s.set(CredentialKey::Session, SECRET).is_err());
    s.remove(CredentialKey::Session);
    enc.set_fail_protect(false);
    s.set(CredentialKey::Session, SECRET).unwrap();
    let restarted = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");
    assert_eq!(
        restarted.get(CredentialKey::Session).as_deref(),
        Some(SECRET)
    );
    assert_eq!(files_containing_secret(dir.path()), Vec::<String>::new());
}

#[test]
fn secure_delete_removes_the_file_and_tolerates_a_missing_one() {
    let dir = TempDir::new().unwrap();
    let p = dir.path().join("x.dat");
    fs::write(&p, vec![7u8; 10_000]).unwrap();
    secure_delete(&p).unwrap();
    assert!(!p.exists());
    secure_delete(&p).unwrap();
}
