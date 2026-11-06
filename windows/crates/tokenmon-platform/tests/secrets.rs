//! Credential store and vault behaviour (spec 01 section 1.6.1 to 1.6.3),
//! ported from `Auth/CredentialStoreTests`. Hermetic: temp dirs and a fake
//! encryptor.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;
use tokenmon_core::{CredentialKey, CredentialStore};
use tokenmon_platform::secrets::{
    CryptError, Encryptor, FakeEncryptor, FallbackPolicy, FileBackedCredentialStore,
    PENDING_FILE_NAME, PayloadError, SecretRoutingCredentialStore, VAULT_FILE_NAME, Vault,
    VaultPayload, decode_payload, decode_pending, encode_payload,
};
use zeroize::Zeroizing;

const SECRET: &str = "sessionKey=sk-ant-VERY-SECRET; other=1";

fn vault_in(dir: &Path, enc: &Arc<FakeEncryptor>) -> Arc<Vault> {
    Arc::new(Vault::new(dir, enc.clone()))
}

fn store(dir: &Path, vault: &Arc<Vault>, prefix: &str) -> SecretRoutingCredentialStore {
    SecretRoutingCredentialStore::new(vault.clone(), dir, prefix, FallbackPolicy::PlainFile)
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn file_store_delegates_and_round_trips() {
    let dir = TempDir::new().unwrap();
    let s = FileBackedCredentialStore::new(dir.path(), "auth_");
    assert_eq!(s.get(CredentialKey::Email), None);
    s.set(CredentialKey::Email, "a@b.com").unwrap();
    assert_eq!(s.get(CredentialKey::Email).as_deref(), Some("a@b.com"));
    assert!(dir.path().join("auth_email.dat").exists());
    s.remove(CredentialKey::Email);
    s.remove(CredentialKey::Email); // missing is fine
    assert_eq!(s.get(CredentialKey::Email), None);
}

#[cfg(unix)]
#[test]
fn file_permissions_are_user_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let s = FileBackedCredentialStore::new(dir.path(), "auth_");
    s.set(CredentialKey::Account, "x").unwrap();
    let mode = fs::metadata(s.path(CredentialKey::Account))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn file_store_first_write_is_atomic_and_leaves_no_staging_file() {
    let dir = TempDir::new().unwrap();
    let s = FileBackedCredentialStore::new(dir.path(), "activity_");
    s.set(CredentialKey::Workspace, "w").unwrap();
    s.set(CredentialKey::Workspace, "w2").unwrap();
    assert_eq!(names(dir.path()), ["activity_workspace.dat"]);
}

#[test]
fn file_store_reports_failed_write_for_missing_directory() {
    let dir = TempDir::new().unwrap();
    let s = FileBackedCredentialStore::new(dir.path().join("nope"), "a_");
    assert!(s.set(CredentialKey::Email, "x").is_err());
    assert!(!dir.path().join("nope").exists());
}

#[test]
fn migration_moves_secret_file_into_vault_and_email_stays() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let plain = FileBackedCredentialStore::new(dir.path(), "claude_auth_");
    plain.set(CredentialKey::Session, SECRET).unwrap();
    plain.set(CredentialKey::Email, "a@b.com").unwrap();

    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");
    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some(SECRET));
    assert!(!plain.path(CredentialKey::Session).exists());
    assert!(plain.path(CredentialKey::Email).exists());
    assert!(dir.path().join(VAULT_FILE_NAME).exists());
}

#[test]
fn migration_failure_keeps_file_and_keeps_working() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let plain = FileBackedCredentialStore::new(dir.path(), "claude_auth_");
    plain.set(CredentialKey::Session, SECRET).unwrap();
    enc.set_fail_protect(true);

    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");
    assert!(plain.path(CredentialKey::Session).exists());
    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some(SECRET));
}

#[test]
fn secure_write_removes_leftover_file() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "grok_auth_");
    FileBackedCredentialStore::new(dir.path(), "grok_auth_")
        .set(CredentialKey::Session, "old")
        .unwrap();
    s.set(CredentialKey::Session, "new").unwrap();
    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some("new"));
    assert!(!dir.path().join("grok_auth_session.dat").exists());
}

#[test]
fn secret_is_never_written_in_plaintext() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "claude_auth_");
    s.set(CredentialKey::Session, SECRET).unwrap();
    for name in names(dir.path()) {
        let bytes = fs::read(dir.path().join(name)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("VERY-SECRET"));
    }
}

#[test]
fn providers_share_one_vault_file() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let vault = vault_in(dir.path(), &enc);
    let a = store(dir.path(), &vault, "claude_auth_");
    let b = store(dir.path(), &vault, "chatgpt_auth_");
    a.set(CredentialKey::Session, "A").unwrap();
    b.set(CredentialKey::Session, "B").unwrap();
    assert_eq!(names(dir.path()), [VAULT_FILE_NAME]);
    assert_eq!(a.get(CredentialKey::Session).as_deref(), Some("A"));
    assert_eq!(b.get(CredentialKey::Session).as_deref(), Some("B"));
    // OpenRouter's ApiKey lives in the same vault.
    let o = store(dir.path(), &vault, "openrouter_auth_");
    o.set(CredentialKey::ApiKey, "K").unwrap();
    assert_eq!(names(dir.path()), [VAULT_FILE_NAME]);
}

struct Counting {
    inner: FakeEncryptor,
    reads: AtomicUsize,
}

impl Encryptor for Counting {
    fn protect(&self, p: &[u8]) -> Result<Vec<u8>, CryptError> {
        self.inner.protect(p)
    }
    fn unprotect(&self, b: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.unprotect(b)
    }
}

#[test]
fn vault_caches_reads() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(Counting {
        inner: FakeEncryptor::default(),
        reads: AtomicUsize::new(0),
    });
    Vault::new(dir.path(), enc.clone()).set("v", "p_session");
    let v = Vault::new(dir.path(), enc.clone());
    enc.reads.store(0, Ordering::SeqCst);
    for _ in 0..5 {
        assert_eq!(v.value("p_session").as_deref(), Some("v"));
    }
    assert_eq!(enc.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn refused_vault_is_never_overwritten() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    assert!(vault_in(dir.path(), &enc).set("keep", "a_session"));
    let before = fs::read(dir.path().join(VAULT_FILE_NAME)).unwrap();

    enc.set_fail_unprotect(true);
    let v = vault_in(dir.path(), &enc);
    assert_eq!(v.value("a_session"), None);
    assert!(v.is_unavailable());
    enc.set_fail_unprotect(false); // locked out even after recovery
    assert!(!v.set("clobber", "a_session"));
    assert_eq!(v.value("a_session"), None);
    assert_eq!(fs::read(dir.path().join(VAULT_FILE_NAME)).unwrap(), before);
}

#[test]
fn undecodable_vault_is_unavailable_not_replaced() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let junk = enc.protect(b"{not json").unwrap();
    fs::write(dir.path().join(VAULT_FILE_NAME), &junk).unwrap();
    let v = vault_in(dir.path(), &enc);
    assert!(!v.set("x", "a_session"));
    assert_eq!(fs::read(dir.path().join(VAULT_FILE_NAME)).unwrap(), junk);
}

#[test]
fn oversized_vault_file_is_unavailable() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    fs::write(dir.path().join(VAULT_FILE_NAME), vec![0u8; 9 * 1024 * 1024]).unwrap();
    assert!(vault_in(dir.path(), &enc).is_unavailable());
}

#[test]
fn removal_while_vault_unavailable_is_applied_on_next_load() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let v = vault_in(dir.path(), &enc);
    assert!(v.set("s1", "a_session"));
    assert!(v.set("s2", "b_session"));

    enc.set_fail_unprotect(true);
    let locked = vault_in(dir.path(), &enc);
    locked.remove("a_session");
    assert!(dir.path().join(PENDING_FILE_NAME).exists());

    enc.set_fail_unprotect(false);
    let next = vault_in(dir.path(), &enc);
    assert_eq!(next.value("a_session"), None);
    assert_eq!(next.value("b_session").as_deref(), Some("s2"));
    assert!(!dir.path().join(PENDING_FILE_NAME).exists());
}

#[test]
fn pending_removals_survive_a_failed_rewrite() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    assert!(vault_in(dir.path(), &enc).set("s1", "a_session"));
    fs::write(
        dir.path().join(PENDING_FILE_NAME),
        br#"["a_session","a_session"]"#,
    )
    .unwrap();

    enc.set_fail_protect(true);
    let v = vault_in(dir.path(), &enc);
    // Load succeeds but the rewrite fails: the record is kept for later.
    assert_eq!(v.value("a_session").as_deref(), Some("s1"));
    assert!(dir.path().join(PENDING_FILE_NAME).exists());

    enc.set_fail_protect(false);
    assert_eq!(vault_in(dir.path(), &enc).value("a_session"), None);
    assert!(!dir.path().join(PENDING_FILE_NAME).exists());
}

#[test]
fn failed_write_is_reported() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let vault = vault_in(dir.path(), &enc);
    enc.set_fail_protect(true);
    assert!(!vault.set("x", "a_session"));
    let refuse =
        SecretRoutingCredentialStore::new(vault.clone(), dir.path(), "a_", FallbackPolicy::Refuse);
    assert!(refuse.set(CredentialKey::Session, "x").is_err());
    assert_eq!(refuse.get(CredentialKey::Session), None);
    assert!(!dir.path().join("a_session.dat").exists());
}

#[test]
fn failed_vault_write_falls_back_to_plain_file_then_migrates() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    enc.set_fail_protect(true);
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "a_");
    s.set(CredentialKey::Session, "x").unwrap();
    assert!(dir.path().join("a_session.dat").exists());

    enc.set_fail_protect(false);
    let s2 = store(dir.path(), &vault_in(dir.path(), &enc), "a_");
    assert!(!dir.path().join("a_session.dat").exists());
    assert_eq!(s2.get(CredentialKey::Session).as_deref(), Some("x"));
}

/// Encrypts to something that decrypts to different content.
struct Corrupting;

impl Encryptor for Corrupting {
    fn protect(&self, _: &[u8]) -> Result<Vec<u8>, CryptError> {
        Ok(vec![1, 2, 3])
    }
    fn unprotect(&self, _: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptError> {
        Ok(Zeroizing::new(br#"{"entries":{}}"#.to_vec()))
    }
}

#[test]
fn vault_verifies_write_by_reading_back_and_keeps_file_on_corruption() {
    let dir = TempDir::new().unwrap();
    let vault = Arc::new(Vault::new(dir.path(), Arc::new(Corrupting)));
    let s = store(dir.path(), &vault, "a_");
    // The write "succeeds" but reads back different: report failure, fall back.
    s.set(CredentialKey::Session, "x").unwrap();
    assert!(dir.path().join("a_session.dat").exists());
    assert_eq!(s.get(CredentialKey::Session).as_deref(), Some("x"));
}

#[test]
fn remove_clears_vault_and_file() {
    let dir = TempDir::new().unwrap();
    let enc = Arc::new(FakeEncryptor::default());
    let s = store(dir.path(), &vault_in(dir.path(), &enc), "a_");
    s.set(CredentialKey::Session, "x").unwrap();
    s.set(CredentialKey::Email, "e").unwrap();
    s.remove(CredentialKey::Session);
    s.remove(CredentialKey::Email);
    assert_eq!(s.get(CredentialKey::Session), None);
    assert_eq!(s.get(CredentialKey::Email), None);
}

#[test]
fn payload_decoder_contract() {
    let p =
        decode_payload(br#"{"entries":{"a_session":"v"},"checkedSeparateItems":["x"]}"#).unwrap();
    assert_eq!(p.entries["a_session"].as_str(), "v");
    assert_eq!(decode_payload(b"[]"), Err(PayloadError::Malformed));
    assert_eq!(decode_payload(b""), Err(PayloadError::Malformed));
    assert_eq!(
        decode_payload(br#"{"entries":{"a":1}}"#),
        Err(PayloadError::Malformed)
    );
    let big = vec![b' '; 4 * 1024 * 1024 + 1];
    assert_eq!(decode_payload(&big), Err(PayloadError::TooLarge));
    assert!(decode_pending(b"[\"a\",\"a\",\"b\"]").unwrap().len() == 2);
    assert!(decode_pending(b"{}").is_err());
}

#[test]
fn payload_round_trips_awkward_secrets_and_never_debug_prints_them() {
    let mut p = VaultPayload::default();
    p.entries.insert(
        "k\"1".into(),
        Zeroizing::new("a\"b\\c\n\u{1}\u{e9}\u{1F600}".into()),
    );
    let back = decode_payload(&encode_payload(&p)).unwrap();
    assert_eq!(back, p);
    assert!(!format!("{p:?}").contains('\u{e9}'));
    assert!(!format!("{p:?}").contains("a\\\"b"));
}

#[cfg(windows)]
#[test]
fn dpapi_round_trip() {
    use tokenmon_platform::secrets::DpapiEncryptor;
    let e = DpapiEncryptor;
    let blob = e.protect(b"hello").unwrap();
    assert_ne!(blob, b"hello");
    assert_eq!(e.unprotect(&blob).unwrap().as_slice(), b"hello");
    assert!(e.unprotect(b"garbage").is_err());
}
