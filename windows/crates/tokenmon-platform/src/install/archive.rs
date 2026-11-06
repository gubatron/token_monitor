//! Bounded, traversal-safe zip extraction.

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use tokenmon_core::update::InstallError;

use super::EXE_NAME;

/// Most entries an archive may hold.
const MAX_ENTRIES: usize = 10_000;

fn archive_err(msg: &str) -> InstallError {
    InstallError::Archive(msg.to_owned())
}

/// Extracts `zip_path` into `dest`, refusing anything suspicious.
///
/// Refused: symbolic links, entries that escape `dest` (`..`, absolute paths,
/// drive letters, backslashes, alternate data streams), more than 10 000
/// entries, and archives whose declared or actual uncompressed size exceeds
/// `max_total` bytes (the actual size is enforced while copying, so a lying
/// header cannot bypass it).
///
/// # Errors
///
/// [`InstallError::Archive`] with a user-safe message.
pub fn extract_zip(zip_path: &Path, dest: &Path, max_total: u64) -> Result<(), InstallError> {
    let file =
        File::open(zip_path).map_err(|_| archive_err("The release zip could not be opened."))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|_| archive_err("The release zip could not be opened."))?;
    if zip.len() > MAX_ENTRIES {
        return Err(archive_err("The release zip has too many files."));
    }
    let declared = (0..zip.len()).try_fold(0u64, |acc, i| {
        let entry = zip
            .by_index_raw(i)
            .map_err(|_| archive_err("The release zip could not be read."))?;
        Ok::<u64, InstallError>(acc.saturating_add(entry.size()))
    })?;
    if declared > max_total {
        return Err(archive_err("The release zip is unexpectedly large."));
    }
    fs::create_dir_all(dest).map_err(|_| archive_err("The release zip could not be read."))?;
    let mut remaining = max_total;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|_| archive_err("The release zip could not be read."))?;
        if entry.is_symlink() || is_link_mode(entry.unix_mode()) {
            return Err(archive_err("The release zip contains a symbolic link."));
        }
        let relative = safe_relative_path(entry.name(), entry.enclosed_name())?;
        let target = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|_| archive_err("The release zip could not be read."))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| archive_err("The release zip could not be read."))?;
        }
        let mut out =
            File::create(&target).map_err(|_| archive_err("The release zip could not be read."))?;
        let written = io::copy(
            &mut (&mut entry).take(remaining.saturating_add(1)),
            &mut out,
        )
        .map_err(|_| archive_err("The release zip could not be read."))?;
        if written > remaining {
            return Err(archive_err("The release zip is unexpectedly large."));
        }
        remaining -= written;
    }
    Ok(())
}

fn is_link_mode(mode: Option<u32>) -> bool {
    mode.is_some_and(|m| m & 0o170_000 == 0o120_000)
}

/// Accepts only plain relative names (no drive letters, streams or backslashes).
fn safe_relative_path(raw_name: &str, enclosed: Option<PathBuf>) -> Result<PathBuf, InstallError> {
    let unsafe_name = || archive_err("The release zip contains an unsafe path.");
    if raw_name.contains(['\\', ':']) {
        return Err(unsafe_name());
    }
    enclosed.ok_or_else(unsafe_name)
}

/// The `TokenMon.exe` at the top level of an extracted archive.
///
/// # Errors
///
/// [`InstallError::Archive`] when it is missing, not a regular file, or a link.
pub fn locate_exe(unpacked: &Path) -> Result<PathBuf, InstallError> {
    let exe = unpacked.join(EXE_NAME);
    let missing = || archive_err("The release zip does not contain TokenMon.exe at its top level.");
    let meta = fs::symlink_metadata(&exe).map_err(|_| missing())?;
    if meta.file_type().is_symlink() {
        return Err(archive_err("The release zip contains a symbolic link."));
    }
    if !meta.is_file() {
        return Err(missing());
    }
    Ok(exe)
}

/// Reads at most `limit` bytes of `path`; helper for verification code.
pub(crate) fn read_prefix(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path)?.take(limit).read_to_end(&mut buf)?;
    Ok(buf)
}
