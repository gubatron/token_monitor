//! Atomic file replacement via a same-directory staging file.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::permissions::OwnerOnlyPermissions;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn staging_path(target: &Path) -> io::Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no file name"))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let unique = format!(
        "{}-{}-{nanos:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let staged = format!(".{}.{unique}.tmp", name.to_string_lossy());
    Ok(target.with_file_name(staged))
}

fn create_owner_only(path: &Path) -> io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Writes `contents` to `target` atomically.
///
/// Creates `.<name>.<unique>.tmp` next to the target (owner-only), writes and
/// syncs it, then renames it over the target. On any failure the staging file
/// is removed, so the directory only ever contains the target. A missing parent
/// directory is an error (`NotFound`), never created implicitly.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::{write_atomic, NoopPermissions};
/// let dir = std::env::temp_dir().join(format!("tm-doc-{}", std::process::id()));
/// std::fs::create_dir_all(&dir).unwrap();
/// let file = dir.join("a.dat");
/// write_atomic(&file, b"hello", &NoopPermissions).unwrap();
/// assert_eq!(std::fs::read(&file).unwrap(), b"hello");
/// std::fs::remove_dir_all(&dir).unwrap();
/// ```
pub fn write_atomic(
    target: &Path,
    contents: &[u8],
    permissions: &dyn OwnerOnlyPermissions,
) -> io::Result<()> {
    let staged = staging_path(target)?;
    let result = stage_and_rename(&staged, target, contents, permissions);
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

fn stage_and_rename(
    staged: &Path,
    target: &Path,
    contents: &[u8],
    permissions: &dyn OwnerOnlyPermissions,
) -> io::Result<()> {
    let mut file = create_owner_only(staged)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    permissions.restrict_file(staged)?;
    std::fs::rename(staged, target)
}
