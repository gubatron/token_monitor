//! The OS encryption seam behind the vault.

use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;
use zeroize::Zeroizing;

/// Encryption or decryption failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct CryptError(pub String);

/// Encrypts and decrypts the vault blob for the current user.
pub trait Encryptor: Send + Sync {
    /// Encrypts `plain` (DPAPI `CryptProtectData`, user scope).
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, CryptError>;

    /// Decrypts `blob`; the plaintext is wiped on drop.
    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptError>;
}

const FAKE_MAGIC: &[u8] = b"FAKE1:";

/// Reversible, **insecure** [`Encryptor`] for tests: a magic prefix plus a
/// XOR with `0x5a`, so ciphertext never equals plaintext and garbage is
/// rejected like a failed DPAPI call.
#[derive(Debug, Default)]
pub struct FakeEncryptor {
    fail_protect: AtomicBool,
    fail_unprotect: AtomicBool,
}

impl FakeEncryptor {
    /// Makes [`Encryptor::protect`] fail (or succeed again).
    pub fn set_fail_protect(&self, fail: bool) {
        self.fail_protect.store(fail, Ordering::SeqCst);
    }

    /// Makes [`Encryptor::unprotect`] fail (or succeed again).
    pub fn set_fail_unprotect(&self, fail: bool) {
        self.fail_unprotect.store(fail, Ordering::SeqCst);
    }
}

impl Encryptor for FakeEncryptor {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, CryptError> {
        if self.fail_protect.load(Ordering::SeqCst) {
            return Err(CryptError("injected protect failure".into()));
        }
        let mut out = FAKE_MAGIC.to_vec();
        out.extend(plain.iter().map(|b| b ^ 0x5a));
        Ok(out)
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptError> {
        if self.fail_unprotect.load(Ordering::SeqCst) {
            return Err(CryptError("injected unprotect failure".into()));
        }
        let body = blob
            .strip_prefix(FAKE_MAGIC)
            .ok_or_else(|| CryptError("not a fake-encrypted blob".into()))?;
        Ok(Zeroizing::new(body.iter().map(|b| b ^ 0x5a).collect()))
    }
}
