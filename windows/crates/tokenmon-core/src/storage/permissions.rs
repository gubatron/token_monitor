//! The owner-only permission hook.
//!
//! macOS uses mode 0700/0600. On Unix we do the same; on Windows the data
//! directory gets a DACL with inheritance removed and a single full-control ACE
//! for the current user's SID, which files created inside inherit.
//!
//! # Where the Windows ACL code lives
//!
//! This crate forbids `unsafe`, and setting a SID-based DACL needs the Win32
//! security API. The real implementation is therefore in `tokenmon-platform`
//! (`tokenmon_platform::secrets::WindowsOwnerOnly`) and is installed once at
//! startup with [`register_platform_permissions`]; [`PlatformPermissions`]
//! delegates to it. If nothing was registered (a tool that links only this
//! crate), Windows falls back to `icacls` addressed by absolute path.
//!
//! # Failure policy
//!
//! A failed restriction is logged and the directory keeps the ACL it
//! inherited (see `AppPaths`). That is deliberate: the data directory lives
//! under `%LOCALAPPDATA%`, whose inherited ACL already excludes other
//! accounts, and every credential is protected separately (DPAPI vault, or
//! an owner-only DACL created atomically with the file). Refusing to start
//! because an ACL could not be rewritten (redirected or roaming profiles,
//! policy-locked folders) would turn a hardening step into an outage.

use std::ffi::OsString;
use std::fmt::Debug;
use std::io;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Restricts a directory or file to the current user.
///
/// Injected into [`crate::storage::AppPaths`] and the string store so tests can
/// observe calls and so the OS-specific implementation stays swappable.
pub trait OwnerOnlyPermissions: Send + Sync + Debug {
    /// Makes `path` (a directory) accessible to the current user only.
    fn restrict_directory(&self, path: &Path) -> io::Result<()>;

    /// Makes `path` (a file) accessible to the current user only.
    fn restrict_file(&self, path: &Path) -> io::Result<()>;
}

static REGISTERED: OnceLock<Arc<dyn OwnerOnlyPermissions>> = OnceLock::new();

/// Installs the platform's real owner-only implementation for the process.
///
/// Returns `false` (and changes nothing) when one is already installed.
/// [`PlatformPermissions`] uses it on every OS once set.
pub fn register_platform_permissions(permissions: Arc<dyn OwnerOnlyPermissions>) -> bool {
    REGISTERED.set(permissions).is_ok()
}

/// Does nothing. For tests, or platforms where the OS default is acceptable.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPermissions;

impl OwnerOnlyPermissions for NoopPermissions {
    fn restrict_directory(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn restrict_file(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

/// The implementation for the current platform: modes on Unix, an ACL on Windows.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformPermissions;

#[cfg(unix)]
impl OwnerOnlyPermissions for PlatformPermissions {
    fn restrict_directory(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(registered) = REGISTERED.get() {
            return registered.restrict_directory(path);
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }

    fn restrict_file(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(registered) = REGISTERED.get() {
            return registered.restrict_file(path);
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
}

/// Absolute path of `icacls.exe` under the system directory taken from
/// `system_root` (`%SystemRoot%`); `C:\Windows` when that is missing or not
/// an absolute drive path. Never a bare name, so the application directory
/// cannot shadow it.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::icacls_program;
/// assert_eq!(icacls_program(Some(r"D:\Win")), r"D:\Win\System32\icacls.exe");
/// assert_eq!(icacls_program(Some("icacls")), r"C:\Windows\System32\icacls.exe");
/// assert_eq!(icacls_program(None), r"C:\Windows\System32\icacls.exe");
/// ```
pub fn icacls_program(system_root: Option<&str>) -> String {
    let b = |r: &str| {
        let b = r.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/')
    };
    let root = system_root.filter(|r| b(r)).unwrap_or(r"C:\Windows");
    format!(
        "{}\\System32\\icacls.exe",
        root.trim_end_matches(['\\', '/'])
    )
}

/// Builds the `icacls` arguments that remove inheritance on `path` and grant
/// `user` full control, inherited by children (`(OI)(CI)F`).
///
/// Pure so it is testable on any OS.
///
/// # Examples
///
/// ```
/// use std::path::Path;
/// use tokenmon_core::storage::icacls_args;
/// let args = icacls_args(Path::new(r"C:\Users\a\AppData\Local\TokenMon"), r"PC\a");
/// assert_eq!(args[1], "/inheritance:r");
/// assert_eq!(args[3], r"PC\a:(OI)(CI)F");
/// ```
pub fn icacls_args(path: &Path, user: &str) -> Vec<OsString> {
    vec![
        path.as_os_str().to_owned(),
        OsString::from("/inheritance:r"),
        OsString::from("/grant:r"),
        OsString::from(format!("{user}:(OI)(CI)F")),
    ]
}

#[cfg(windows)]
impl PlatformPermissions {
    fn current_user() -> io::Result<String> {
        let name = std::env::var("USERNAME")
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "USERNAME is not set"))?;
        Ok(match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
            _ => name,
        })
    }
}

#[cfg(windows)]
impl OwnerOnlyPermissions for PlatformPermissions {
    fn restrict_directory(&self, path: &Path) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if let Some(registered) = REGISTERED.get() {
            return registered.restrict_directory(path);
        }
        // Fallback when the platform layer did not register its SID-based
        // implementation: absolute icacls, user name from the environment.
        let user = Self::current_user()?;
        let program = icacls_program(std::env::var("SystemRoot").ok().as_deref());
        let status = std::process::Command::new(program)
            .args(icacls_args(path, &user))
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("icacls exited with {status}")))
        }
    }

    /// Files inherit the owner-only ACL from their restricted directory
    /// unless a registered implementation applies one explicitly.
    fn restrict_file(&self, path: &Path) -> io::Result<()> {
        match REGISTERED.get() {
            Some(registered) => registered.restrict_file(path),
            None => Ok(()),
        }
    }
}

#[cfg(not(any(unix, windows)))]
impl OwnerOnlyPermissions for PlatformPermissions {
    fn restrict_directory(&self, path: &Path) -> io::Result<()> {
        REGISTERED
            .get()
            .map_or(Ok(()), |r| r.restrict_directory(path))
    }

    fn restrict_file(&self, path: &Path) -> io::Result<()> {
        REGISTERED.get().map_or(Ok(()), |r| r.restrict_file(path))
    }
}
