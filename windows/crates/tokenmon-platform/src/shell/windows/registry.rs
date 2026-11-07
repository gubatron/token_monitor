//! [`Registry`] over `HKEY_CURRENT_USER` (Win32 `Reg*` calls).
//!
//! A thin adapter excluded from coverage (`windows/.config/coverage-excludes.txt`):
//! tests never touch the real registry. The `REG_SZ` byte format lives in the
//! portable [`crate::shell::registry`] ([`encode_reg_sz`], [`decode_reg_sz`]),
//! and the key layout and decisions in `autostart` and `context_menu`, all
//! tested against [`MemoryRegistry`](crate::shell::MemoryRegistry).

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_BINARY, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};

use super::wide;
use crate::shell::error::ShellError;
use crate::shell::registry::{Registry, decode_reg_sz, encode_reg_sz};

fn reg_err(op: &str, code: u32) -> ShellError {
    ShellError::Registry(format!("{op} failed with Win32 error {code}"))
}

fn is_missing(code: u32) -> bool {
    code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND
}

/// Owns an open registry key and closes it on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpen/RegCreate and is closed once.
        unsafe { RegCloseKey(self.0) };
    }
}

/// [`Registry`] over `HKEY_CURRENT_USER`.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinRegistry;

impl WinRegistry {
    fn open(key: &str, access: u32) -> Result<Option<Key>, ShellError> {
        let key_w = wide(key);
        let mut out: HKEY = null_mut();
        // SAFETY: `key_w` is NUL-terminated and outlives the call; `out` is a valid out-pointer.
        let rc =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_w.as_ptr(), 0, access, &raw mut out) };
        match rc {
            ERROR_SUCCESS => Ok(Some(Key(out))),
            c if is_missing(c) => Ok(None),
            c => Err(reg_err("RegOpenKeyExW", c)),
        }
    }

    fn create(key: &str) -> Result<Key, ShellError> {
        let key_w = wide(key);
        let mut out: HKEY = null_mut();
        // SAFETY: all pointers are valid for the call; null class/security/disposition are allowed.
        let rc = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key_w.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE | KEY_READ,
                null(),
                &raw mut out,
                null_mut(),
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(Key(out))
        } else {
            Err(reg_err("RegCreateKeyExW", rc))
        }
    }

    fn query(key: &str, name: &str, want: u32) -> Result<Option<Vec<u8>>, ShellError> {
        let Some(k) = Self::open(key, KEY_READ)? else {
            return Ok(None);
        };
        let name_w = wide(name);
        let mut kind = 0u32;
        let mut len = 0u32;
        // SAFETY: first call with a null buffer only asks for the size.
        let rc = unsafe {
            RegQueryValueExW(
                k.0,
                name_w.as_ptr(),
                null(),
                &raw mut kind,
                null_mut(),
                &raw mut len,
            )
        };
        if is_missing(rc) {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(reg_err("RegQueryValueExW", rc));
        }
        let mut buf = vec![0u8; len as usize];
        // SAFETY: `buf` has `len` writable bytes, matching the size passed in.
        let rc = unsafe {
            RegQueryValueExW(
                k.0,
                name_w.as_ptr(),
                null(),
                &raw mut kind,
                buf.as_mut_ptr(),
                &raw mut len,
            )
        };
        if is_missing(rc) {
            return Ok(None);
        }
        if rc != ERROR_SUCCESS {
            return Err(reg_err("RegQueryValueExW", rc));
        }
        buf.truncate(len as usize);
        Ok((kind == want).then_some(buf))
    }
}

impl Registry for WinRegistry {
    fn get_string(&self, key: &str, name: &str) -> Result<Option<String>, ShellError> {
        Ok(Self::query(key, name, REG_SZ)?.map(|bytes| decode_reg_sz(&bytes)))
    }

    fn set_string(&self, key: &str, name: &str, value: &str) -> Result<(), ShellError> {
        let k = Self::create(key)?;
        let name_w = wide(name);
        let data = encode_reg_sz(value);
        // SAFETY: `data` is valid for `data.len()` bytes and includes the terminating NUL.
        let rc = unsafe {
            RegSetValueExW(
                k.0,
                name_w.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr(),
                data.len() as u32,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(reg_err("RegSetValueExW", rc))
        }
    }

    fn get_binary(&self, key: &str, name: &str) -> Result<Option<Vec<u8>>, ShellError> {
        Self::query(key, name, REG_BINARY)
    }

    fn delete_value(&self, key: &str, name: &str) -> Result<(), ShellError> {
        let Some(k) = Self::open(key, KEY_WRITE)? else {
            return Ok(());
        };
        let name_w = wide(name);
        // SAFETY: `name_w` is NUL-terminated and `k` is an open key.
        let rc = unsafe { RegDeleteValueW(k.0, name_w.as_ptr()) };
        if rc == ERROR_SUCCESS || is_missing(rc) {
            Ok(())
        } else {
            Err(reg_err("RegDeleteValueW", rc))
        }
    }

    fn delete_key_tree(&self, key: &str) -> Result<(), ShellError> {
        let key_w = wide(key);
        // SAFETY: `key_w` is NUL-terminated and outlives the call.
        let rc = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key_w.as_ptr()) };
        if rc == ERROR_SUCCESS || is_missing(rc) {
            Ok(())
        } else {
            Err(reg_err("RegDeleteTreeW", rc))
        }
    }

    fn key_exists(&self, key: &str) -> Result<bool, ShellError> {
        Ok(Self::open(key, KEY_READ)?.is_some())
    }
}
