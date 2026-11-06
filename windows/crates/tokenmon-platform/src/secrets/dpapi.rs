//! Windows DPAPI [`Encryptor`] (`CryptProtectData`, current-user scope).

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use zeroize::Zeroizing;

use super::encryptor::{CryptError, Encryptor};

/// Application-specific entropy: another process of the same user must also
/// know this constant (defence in depth only, it is not a secret).
const ENTROPY: &[u8] = b"TokenMon.vault.v1";

/// DPAPI user-scope encryption bound to the logged-in Windows account.
#[derive(Debug, Default, Clone, Copy)]
pub struct DpapiEncryptor;

fn blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, CryptError> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).map_err(|_| CryptError("blob too large".into()))?,
        pbData: data.as_ptr().cast_mut(),
    })
}

/// Copies the DPAPI output into a Rust buffer, wipes and frees the OS one.
///
/// # Safety
/// `out` must be a blob filled in by a successful DPAPI call.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Zeroizing<Vec<u8>> {
    // SAFETY: per the contract, `pbData` points to `cbData` readable bytes
    // allocated with LocalAlloc.
    let slice = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) };
    let copy = Zeroizing::new(slice.to_vec());
    // SAFETY: we own the buffer until LocalFree, so wiping it is sound, and
    // it was allocated by LocalAlloc, so LocalFree is the matching deallocator.
    unsafe {
        std::ptr::write_bytes(out.pbData, 0, out.cbData as usize);
        let _ = LocalFree(out.pbData.cast());
    }
    copy
}

impl Encryptor for DpapiEncryptor {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>, CryptError> {
        let input = blob(plain)?;
        let entropy = blob(ENTROPY)?;
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // SAFETY: `input` and `entropy` point to live slices for the call;
        // `out` is a valid out-parameter. UI is forbidden so it never prompts.
        let ok = unsafe {
            CryptProtectData(
                &raw const input,
                null(),
                &raw const entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut out,
            )
        };
        if ok == 0 {
            // SAFETY: GetLastError has no preconditions.
            let code = unsafe { GetLastError() };
            return Err(CryptError(format!("CryptProtectData failed: error {code}")));
        }
        // SAFETY: the call above succeeded, so `out` is a valid DPAPI blob.
        Ok(unsafe { take(out) }.to_vec())
    }

    fn unprotect(&self, data: &[u8]) -> Result<Zeroizing<Vec<u8>>, CryptError> {
        let input = blob(data)?;
        let entropy = blob(ENTROPY)?;
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // SAFETY: as in `protect`.
        let ok = unsafe {
            CryptUnprotectData(
                &raw const input,
                null(),
                &raw const entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut out,
            )
        };
        if ok == 0 {
            // SAFETY: GetLastError has no preconditions.
            let code = unsafe { GetLastError() };
            return Err(CryptError(format!("CryptUnprotectData failed: error {code}")));
        }
        // SAFETY: the call above succeeded, so `out` is a valid DPAPI blob.
        Ok(unsafe { take(out) })
    }
}
