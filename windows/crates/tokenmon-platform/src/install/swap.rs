//! Staged replacement of the running executable.
//!
//! Windows lets a running exe be *renamed* but not overwritten or deleted, so
//! the swap is: copy the new exe next to the old one (same volume), rename the
//! running exe aside, rename the staged copy into its place. If the second
//! rename fails the first is undone, leaving the original in place.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::{Builder, TempPath};
use tokenmon_core::update::InstallError;

fn install_err(e: impl std::fmt::Display) -> InstallError {
    InstallError::Install(e.to_string())
}

/// Whether the running executable can be replaced in place: it is a regular
/// `.exe` file and its folder accepts new files.
///
/// # Examples
///
/// ```
/// use tokenmon_platform::install::swap::can_replace_running_exe;
///
/// assert!(!can_replace_running_exe(std::path::Path::new("/nonexistent/TokenMon.exe")));
/// ```
pub fn can_replace_running_exe(exe: &Path) -> bool {
    let is_exe = exe
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    let Some(parent) = exe.parent() else {
        return false;
    };
    is_exe
        && fs::symlink_metadata(exe).is_ok_and(|m| m.is_file())
        && Builder::new()
            .prefix(".tokenmon-write-probe-")
            .tempfile_in(parent)
            .is_ok()
}

/// A copy of the new exe sitting beside the running one. Deleted on drop
/// unless [`commit_swap`] consumed it.
#[derive(Debug)]
pub struct StagedExe {
    path: TempPath,
}

impl StagedExe {
    /// Where the staged copy lives.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Copies `new_exe` to a uniquely named hidden sibling of `running`.
///
/// # Errors
///
/// [`InstallError::Install`] when the copy fails; no staged file is left.
pub fn stage_next_to(new_exe: &Path, running: &Path) -> Result<StagedExe, InstallError> {
    let parent = running
        .parent()
        .ok_or_else(|| install_err("The executable has no folder."))?;
    let staged = Builder::new()
        .prefix(".TokenMon-update-")
        .suffix(".exe")
        .tempfile_in(parent)
        .map_err(install_err)?
        .into_temp_path();
    fs::copy(new_exe, &staged).map_err(install_err)?;
    Ok(StagedExe { path: staged })
}

/// Prefix of the name the replaced exe is renamed to: `TokenMon.old-`.
fn aside_prefix(running: &Path) -> String {
    let stem = running
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}.old-")
}

/// A name that is unique per call (process id plus a nanosecond stamp).
fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}{:x}", std::process::id(), nanos)
}

/// Path the replaced exe is renamed to (`TokenMon.old-<unique>.exe`).
fn aside_path(running: &Path) -> PathBuf {
    running.with_file_name(format!("{}{}.exe", aside_prefix(running), unique_suffix()))
}

/// Renames `running` aside and moves `staged` into its place.
///
/// Returns the aside path (deleted by [`cleanup_leftovers`] on the next
/// launch; it may still be locked by the running process now).
///
/// # Errors
///
/// [`InstallError::Install`]; in every error case `running` is back in place.
pub fn commit_swap(staged: StagedExe, running: &Path) -> Result<PathBuf, InstallError> {
    let aside = aside_path(running);
    fs::rename(running, &aside).map_err(install_err)?;
    if let Err(e) = staged.path.persist(running) {
        // Best effort: put the original back so the app still launches.
        let _ = fs::rename(&aside, running);
        return Err(install_err(e.error));
    }
    Ok(aside)
}

/// Removes leftovers of earlier updates beside `running`: renamed-aside exes
/// (`*.old-*.exe`) and abandoned staged copies. Call once at startup; failures
/// (for example a file still locked) are ignored and retried next launch.
pub fn cleanup_leftovers(running: &Path) {
    let Some(parent) = running.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let prefix = aside_prefix(running);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale_aside = name.starts_with(&prefix) && name.to_ascii_lowercase().ends_with(".exe");
        if stale_aside || name.starts_with(".TokenMon-update-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}
