//! The version a downloaded binary declares about itself (#77).
//!
//! The release tag is attacker-influenced input when an old, validly digested
//! build is replayed under a newer tag, so the installer reads the version the
//! downloaded file carries instead. For an `.exe` that is the `VS_VERSIONINFO`
//! resource, parsed here without any OS call so the logic runs (and is fuzzed)
//! on every host. For an `.msi` it is the `ProductVersion` row of the
//! `Property` table, which only `msi.dll` reads; see [`SystemVersionReader`].
//!
//! Everything here is total over arbitrary bytes: every read is bounds-checked
//! against the file length, directory walks are depth- and count-limited, and
//! the version blob is capped, so hostile input yields `None`, never a panic
//! or an unbounded allocation.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tokenmon_core::update::{AppVersion, InstallError};

/// Largest `VS_VERSIONINFO` blob read (real ones are about 1 KiB).
const MAX_BLOB: u32 = 64 * 1024;
/// Most sections and resource-directory entries visited.
const MAX_ENTRIES: u32 = 256;
/// `RT_VERSION`.
const RT_VERSION: u32 = 16;
/// `VS_FIXEDFILEINFO.dwSignature`.
const FIXED_SIGNATURE: u32 = 0xFEEF_04BD;

/// Reads the version a downloaded installer artifact declares.
pub trait PackageVersionReader: Send + Sync {
    /// The version in the executable's version resource.
    fn exe_version(&self, path: &Path) -> Result<AppVersion, InstallError>;
    /// The `ProductVersion` of the MSI package.
    fn msi_version(&self, path: &Path) -> Result<AppVersion, InstallError>;
}

/// The production reader: the portable PE parser for executables and
/// `msi.dll` for packages (fails closed on hosts without it).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemVersionReader;

fn unreadable(what: &str) -> InstallError {
    InstallError::Verification(format!("The {what} does not declare a readable version."))
}

impl PackageVersionReader for SystemVersionReader {
    fn exe_version(&self, path: &Path) -> Result<AppVersion, InstallError> {
        let file = std::fs::File::open(path).map_err(|_| unreadable("download"))?;
        read_pe_version(file).ok_or_else(|| unreadable("download"))
    }

    fn msi_version(&self, path: &Path) -> Result<AppVersion, InstallError> {
        msi::product_version(path)
            .and_then(|text| AppVersion::parse(&text))
            .ok_or_else(|| unreadable("installer"))
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Bounds-checked random access to the image.
struct Image<R> {
    r: R,
    len: u64,
}

impl<R: Read + Seek> Image<R> {
    fn read(&mut self, at: u64, n: usize) -> Option<Vec<u8>> {
        if at.checked_add(n as u64)? > self.len {
            return None;
        }
        self.r.seek(SeekFrom::Start(at)).ok()?;
        let mut buf = vec![0u8; n];
        self.r.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

struct Section {
    va: u32,
    raw_size: u32,
    raw_ptr: u32,
}

fn rva_to_offset(sections: &[Section], rva: u32) -> Option<u64> {
    sections.iter().find_map(|s| {
        let delta = rva.checked_sub(s.va)?;
        (delta < s.raw_size).then(|| u64::from(s.raw_ptr) + u64::from(delta))
    })
}

/// The version a PE image's `VS_VERSIONINFO` resource declares, preferring the
/// `ProductVersion` string (it can carry a prerelease suffix) over the fixed
/// 4-part file version. `None` when the image has no readable version.
///
/// # Examples
///
/// ```
/// use std::io::Cursor;
/// use tokenmon_platform::install::pe_version::read_pe_version;
///
/// assert!(read_pe_version(Cursor::new(b"MZ".to_vec())).is_none());
/// assert!(read_pe_version(Cursor::new(Vec::new())).is_none());
/// ```
pub fn read_pe_version<R: Read + Seek>(mut r: R) -> Option<AppVersion> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    let mut img = Image { r, len };
    let dos = img.read(0, 64)?;
    if dos.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u64::from(u32_at(&dos, 0x3C)?);
    let head = img.read(pe, 24)?;
    if head.get(..4)? != b"PE\0\0" {
        return None;
    }
    let n_sections = u32::from(u16_at(&head, 6)?);
    let opt_size = usize::from(u16_at(&head, 20)?);
    let opt = pe + 24;
    let (dd_at, count_at) = match u16_at(&img.read(opt, 2)?, 0)? {
        0x10B => (96usize, 92usize),
        0x20B => (112, 108),
        _ => return None,
    };
    if opt_size < dd_at + 24 {
        return None;
    }
    let optional = img.read(opt, dd_at + 24)?;
    if u32_at(&optional, count_at)? < 3 {
        return None;
    }
    let res_rva = u32_at(&optional, dd_at + 16)?;
    let res_size = u32_at(&optional, dd_at + 20)?;
    if res_rva == 0 || res_size == 0 {
        return None;
    }
    let table = img.read(
        opt + opt_size as u64,
        n_sections.min(MAX_ENTRIES) as usize * 40,
    )?;
    let sections: Vec<Section> = table
        .as_chunks::<40>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Section {
                va: u32_at(c, 12)?,
                raw_size: u32_at(c, 16)?,
                raw_ptr: u32_at(c, 20)?,
            })
        })
        .collect();
    let base = rva_to_offset(&sections, res_rva)?;
    let (rva, size) = find_version_blob(&mut img, base)?;
    let offset = rva_to_offset(&sections, rva)?;
    let blob = img.read(offset, size.min(MAX_BLOB) as usize)?;
    parse_version_info(&blob)
}

/// Walks type -> name -> language and returns the `(rva, size)` of the first
/// `RT_VERSION` data entry.
fn find_version_blob<R: Read + Seek>(img: &mut Image<R>, base: u64) -> Option<(u32, u32)> {
    let mut dir = base;
    for level in 0..3 {
        let header = img.read(dir, 16)?;
        let named = u32::from(u16_at(&header, 12)?);
        let ids = u32::from(u16_at(&header, 14)?);
        let total = (named + ids).min(MAX_ENTRIES);
        let entries = img.read(dir + 16, total as usize * 8)?;
        let pick = entries.as_chunks::<8>().0.iter().find_map(|e| {
            let name = u32_at(e, 0)?;
            let target = u32_at(e, 4)?;
            // Only the type level is filtered (by integer id).
            let wanted = level != 0 || (name & 0x8000_0000 == 0 && name == RT_VERSION);
            wanted.then_some(target)
        })?;
        let is_dir = pick & 0x8000_0000 != 0;
        let next = base + u64::from(pick & 0x7FFF_FFFF);
        if level == 2 {
            if is_dir {
                return None;
            }
            let data = img.read(next, 16)?;
            return Some((u32_at(&data, 0)?, u32_at(&data, 4)?));
        }
        if !is_dir {
            return None;
        }
        dir = next;
    }
    None
}

/// One `VS_VERSIONINFO`-style node: `wLength, wValueLength, wType, key, value,
/// children`.
struct Node<'a> {
    key: String,
    text: bool,
    value: &'a [u8],
    children: &'a [u8],
    /// Offset just past this node (4-aligned) within the parent slice.
    next: usize,
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}

fn node(buf: &[u8], pos: usize) -> Option<Node<'_>> {
    let len = usize::from(u16_at(buf, pos)?);
    let value_len = usize::from(u16_at(buf, pos + 2)?);
    let text = u16_at(buf, pos + 4)? == 1;
    let end = pos.checked_add(len)?;
    if len < 6 || end > buf.len() {
        return None;
    }
    let mut at = pos + 6;
    let mut key = Vec::new();
    loop {
        let unit = u16_at(buf, at)?;
        at += 2;
        if unit == 0 {
            break;
        }
        key.push(unit);
        if key.len() > 64 || at > end {
            return None;
        }
    }
    let value_at = align4(at).min(end);
    let value_bytes = if text { value_len * 2 } else { value_len };
    let value_end = value_at.checked_add(value_bytes)?.min(end);
    Some(Node {
        key: String::from_utf16(&key).ok()?,
        text,
        value: &buf[value_at..value_end],
        children: buf.get(align4(value_end).min(end)..end)?,
        next: align4(end),
    })
}

fn children(buf: &[u8]) -> impl Iterator<Item = Node<'_>> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        let n = node(buf, pos)?;
        pos = n.next;
        Some(n)
    })
}

fn utf16_text(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16(&units).ok()
}

/// Parses a `VS_VERSIONINFO` blob.
fn parse_version_info(blob: &[u8]) -> Option<AppVersion> {
    let root = node(blob, 0)?;
    if root.key != "VS_VERSION_INFO" {
        return None;
    }
    let product = children(root.children)
        .filter(|n| n.key == "StringFileInfo")
        .flat_map(|info| children(info.children).collect::<Vec<_>>())
        .flat_map(|table| children(table.children).collect::<Vec<_>>())
        .find(|s| s.key == "ProductVersion" && s.text)
        .and_then(|s| utf16_text(s.value))
        .and_then(|t| AppVersion::parse(&t));
    product.or_else(|| fixed_version(root.value))
}

fn fixed_version(value: &[u8]) -> Option<AppVersion> {
    if u32_at(value, 0)? != FIXED_SIGNATURE {
        return None;
    }
    let ms = u32_at(value, 8)?;
    let ls = u32_at(value, 12)?;
    AppVersion::parse(&format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xFFFF,
        ls >> 16,
        ls & 0xFFFF
    ))
}

#[cfg(windows)]
mod msi {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::ApplicationInstallationAndServicing::{
        MSIDBOPEN_READONLY, MSIHANDLE, MsiCloseHandle, MsiDatabaseOpenViewW, MsiOpenDatabaseW,
        MsiRecordGetStringW, MsiViewExecute, MsiViewFetch,
    };

    /// Closes the handle on drop.
    struct Handle(MSIHANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful Msi call and is closed once.
            unsafe { MsiCloseHandle(self.0) };
        }
    }

    /// `ProductVersion` from the package's `Property` table, opened read-only.
    pub fn product_version(path: &Path) -> Option<String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let query: Vec<u16> = "SELECT `Value` FROM `Property` WHERE `Property`='ProductVersion'"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut db: MSIHANDLE = 0;
        // SAFETY: `wide` is NUL-terminated and outlives the call; `db` is a valid out pointer.
        if unsafe { MsiOpenDatabaseW(wide.as_ptr(), MSIDBOPEN_READONLY, &raw mut db) }
            != ERROR_SUCCESS
        {
            return None;
        }
        let db = Handle(db);
        let mut view: MSIHANDLE = 0;
        // SAFETY: `query` is NUL-terminated; `view` is a valid out pointer.
        if unsafe { MsiDatabaseOpenViewW(db.0, query.as_ptr(), &raw mut view) } != ERROR_SUCCESS {
            return None;
        }
        let view = Handle(view);
        // SAFETY: `view.0` is an open view; a null record means no parameters.
        if unsafe { MsiViewExecute(view.0, 0) } != ERROR_SUCCESS {
            return None;
        }
        let mut record: MSIHANDLE = 0;
        // SAFETY: `record` is a valid out pointer.
        if unsafe { MsiViewFetch(view.0, &raw mut record) } != ERROR_SUCCESS {
            return None;
        }
        let record = Handle(record);
        // A version is short; a longer value is refused rather than grown into.
        let mut buf = [0u16; 128];
        let mut chars = buf.len() as u32;
        // SAFETY: `buf` holds `chars` UTF-16 units; the API writes at most that many.
        if unsafe { MsiRecordGetStringW(record.0, 1, buf.as_mut_ptr(), &raw mut chars) }
            != ERROR_SUCCESS
        {
            return None;
        }
        String::from_utf16(buf.get(..chars as usize)?).ok()
    }
}

#[cfg(not(windows))]
mod msi {
    use std::path::Path;

    /// No `msi.dll` here: fail closed.
    pub fn product_version(_path: &Path) -> Option<String> {
        None
    }
}
