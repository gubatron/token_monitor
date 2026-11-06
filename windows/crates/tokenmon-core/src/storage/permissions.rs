//! The owner-only permission hook.
//!
//! macOS uses mode 0700/0600. On Unix we do the same; on Windows the data
//! directory gets a DACL with inheritance removed and a single full-control ACE
//! for the current user, which files created inside inherit.

use std::ffi::OsString;
use std::fmt::Debug;
use std::io;
use std::path::Path;

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
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }

    fn restrict_file(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
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
        let user = Self::current_user()?;
        let status = std::process::Command::new("icacls")
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

    /// Files inherit the owner-only ACL from their restricted directory.
    fn restrict_file(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
impl OwnerOnlyPermissions for PlatformPermissions {
    fn restrict_directory(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn restrict_file(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}
