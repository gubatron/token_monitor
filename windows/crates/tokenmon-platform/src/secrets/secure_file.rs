//! Owner-only file creation.
//!
//! The secrets files (`vault.bin`, `vault_pending_removals.json`, the plain
//! `.dat` identity values) must never be readable by another account, not even
//! for the instant between create and a later "restrict" call.
//!
//! * Unix: the file is created with mode 0600 (`open(2)` applies it atomically).
//! * Windows: the file is created with an explicit protected DACL (no inherited
//!   ACEs) that grants only the current user. It is applied inside
//!   `CreateFileW` through `SECURITY_ATTRIBUTES`, so there is no window where
//!   the file carries the inherited ACL. This is why the `icacls` helper in
//!   `tokenmon_core::storage` (a child process run after creation, and one that
//!   depends on `USERNAME`/`USERDOMAIN` environment variables an attacker or a
//!   service account may not have) is not used for secrets. A rename keeps the
//!   DACL, so the staging file's ACL becomes the target's.

use std::fs::File;
use std::io;
use std::path::Path;

/// SDDL for "protected DACL, full access to exactly this SID".
///
/// `P` blocks inheritance from the parent directory; the single ACE grants
/// `FA` (file all access). Pure so it is tested on every OS.
///
/// # Examples
///
/// ```
/// use tokenmon_platform::secrets::owner_only_sddl;
/// assert_eq!(owner_only_sddl("S-1-5-21-1-2-3-1001"), "D:P(A;;FA;;;S-1-5-21-1-2-3-1001)");
/// ```
pub fn owner_only_sddl(sid: &str) -> String {
    format!("D:P(A;;FA;;;{sid})")
}

/// Creates `path` (it must not exist) readable and writable by the current
/// user only, and returns it opened for writing.
pub fn create_new_owner_only(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        windows_impl::create_new(path)
    }
    #[cfg(not(windows))]
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(path)
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::path::Path;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    use super::owner_only_sddl;

    fn last_error() -> io::Error {
        // SAFETY: GetLastError has no preconditions.
        let code = unsafe { GetLastError() };
        io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX))
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The current user's SID as an `S-1-5-...` string.
    fn current_user_sid() -> io::Result<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: valid pseudo-handle, out-pointer to a local.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
            return Err(last_error());
        }
        let result = token_user_sid(token);
        // SAFETY: `token` was opened above and is closed exactly once.
        unsafe { CloseHandle(token) };
        result
    }

    fn token_user_sid(token: HANDLE) -> io::Result<String> {
        let mut len = 0u32;
        // SAFETY: size query with a null buffer; the call fails with
        // ERROR_INSUFFICIENT_BUFFER and fills `len`.
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &raw mut len) };
        if len == 0 {
            return Err(last_error());
        }
        // u64 elements keep the buffer 8-byte aligned for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buf` is at least `len` bytes and suitably aligned.
        if unsafe {
            GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &raw mut len)
        } == 0
        {
            return Err(last_error());
        }
        // SAFETY: on success the buffer begins with a TOKEN_USER.
        let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut wide_sid: *mut u16 = null_mut();
        // SAFETY: `sid` is a valid SID inside `buf`; the out string is
        // LocalAlloc'd and freed below.
        if unsafe { ConvertSidToStringSidW(sid, &raw mut wide_sid) } == 0 {
            return Err(last_error());
        }
        // SAFETY: the string is NUL terminated.
        let text = unsafe {
            let mut n = 0;
            while *wide_sid.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(wide_sid, n))
        };
        // SAFETY: allocated by ConvertSidToStringSidW with LocalAlloc.
        unsafe { LocalFree(wide_sid.cast()) };
        Ok(text)
    }

    pub(super) fn create_new(path: &Path) -> io::Result<File> {
        let sddl = wide(OsStr::new(&owner_only_sddl(&current_user_sid()?)));
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `sddl` is NUL terminated; `descriptor` is freed below.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let name = wide(path.as_os_str());
        // SAFETY: `name` is NUL terminated and `attributes` outlives the call.
        // CREATE_NEW fails if the path exists, so we never adopt a file an
        // attacker pre-created with a permissive ACL.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ,
                &raw const attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        let error = (handle == INVALID_HANDLE_VALUE).then(last_error);
        // SAFETY: allocated by the SDDL conversion with LocalAlloc.
        unsafe { LocalFree(descriptor) };
        match error {
            Some(e) => Err(e),
            // SAFETY: a valid, owned file handle that nothing else closes.
            None => Ok(unsafe { File::from_raw_handle(handle.cast()) }),
        }
    }
}
