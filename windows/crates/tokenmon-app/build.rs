//! Gives `TokenMon.exe` a `VS_VERSIONINFO` resource (#77) and its application
//! icon (#81): `assets/tokenmon.ico` as `RT_ICON` frames plus `RT_GROUP_ICON`
//! id 1, so Explorer, the taskbar, Alt+Tab and Task Manager show the TokenMon
//! logo instead of the generic executable icon.
//!
//! The updater reads the version a downloaded exe declares about itself, so a
//! replayed older build cannot hide behind a newer release tag. The resource is
//! written here as a compiled `.res` file and handed to the MSVC linker, which
//! needs no `rc.exe` and no extra build dependency (so it also works when
//! cross-compiling with `cargo xwin`).
//!
//! The version comes from `CARGO_PKG_VERSION` (the workspace version, the same
//! one the MSI and the release tag carry); the layout is documented at
//! <https://learn.microsoft.com/windows/win32/menurc/vs-versioninfo>.

use std::path::PathBuf;

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn pad4(v: &mut Vec<u8>) {
    v.resize(v.len().div_ceil(4) * 4, 0);
}

/// One version-info node: `wLength, wValueLength, wType, szKey, Padding,
/// Value, Padding, Children`.
fn node(key: &str, text: bool, value_len: usize, value: &[u8], children: &[u8]) -> Vec<u8> {
    let mut n = vec![0u8; 6];
    n.extend(utf16z(key));
    pad4(&mut n);
    n.extend_from_slice(value);
    pad4(&mut n);
    n.extend_from_slice(children);
    let len = u16::try_from(n.len()).unwrap_or(u16::MAX);
    n[..2].copy_from_slice(&len.to_le_bytes());
    n[2..4].copy_from_slice(&u16::try_from(value_len).unwrap_or(0).to_le_bytes());
    n[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    n
}

fn string_entry(name: &str, value: &str) -> Vec<u8> {
    let v = utf16z(value);
    node(name, true, v.len() / 2, &v, &[])
}

fn version_info(version: &str, parts: [u16; 4]) -> Vec<u8> {
    let [a, b, c, d] = parts;
    let ms = (u32::from(a) << 16) | u32::from(b);
    let ls = (u32::from(c) << 16) | u32::from(d);
    let mut fixed = Vec::with_capacity(52);
    for word in [
        0xFEEF_04BD_u32, // dwSignature
        0x0001_0000,     // dwStrucVersion
        ms,              // dwFileVersionMS
        ls,              // dwFileVersionLS
        ms,              // dwProductVersionMS
        ls,              // dwProductVersionLS
        0x3F,            // dwFileFlagsMask
        0,               // dwFileFlags
        0x0004_0004,     // VOS_NT_WINDOWS32
        1,               // VFT_APP
        0,               // dwFileSubtype
        0,               // dwFileDateMS
        0,               // dwFileDateLS
    ] {
        fixed.extend_from_slice(&word.to_le_bytes());
    }
    let mut strings = Vec::new();
    for (k, v) in [
        ("CompanyName", "TokenMon"),
        ("FileDescription", "TokenMon"),
        ("FileVersion", version),
        ("InternalName", "TokenMon"),
        ("OriginalFilename", "TokenMon.exe"),
        ("ProductName", "TokenMon"),
        ("ProductVersion", version),
    ] {
        strings.extend(string_entry(k, v));
        pad4(&mut strings);
    }
    let table = node("040904B0", true, 0, &[], &strings);
    let string_info = node("StringFileInfo", true, 0, &[], &table);
    // Translation: language 0x0409 (US English), code page 0x04B0 (Unicode).
    let translation = node("Translation", false, 4, &[0x09, 0x04, 0xB0, 0x04], &[]);
    let var_info = node("VarFileInfo", true, 0, &[], &translation);
    let mut kids = string_info;
    pad4(&mut kids);
    kids.extend(var_info);
    node("VS_VERSION_INFO", false, fixed.len(), &fixed, &kids)
}

/// `RT_ICON`.
pub const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`.
pub const RT_GROUP_ICON: u16 = 14;
/// `RT_VERSION`.
pub const RT_VERSION: u16 = 16;

/// One resource of a `.res` file, identified by integer type and name ids.
pub struct Entry {
    /// Resource type id (`RT_*`).
    pub kind: u16,
    /// Resource name id.
    pub name: u16,
    /// Memory flags (`MOVEABLE`, `PURE`, `DISCARDABLE`).
    pub flags: u16,
    /// Language id (0 = neutral).
    pub language: u16,
    /// The raw resource data.
    pub data: Vec<u8>,
}

fn le16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// A `.res` file: the mandatory empty first entry, then `entries`.
pub fn res_file(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    le32(&mut out, 0); // DataSize
    le32(&mut out, 32); // HeaderSize
    out.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    out.extend_from_slice(&[0u8; 16]);
    for e in entries {
        le32(&mut out, u32::try_from(e.data.len()).unwrap_or(0));
        le32(&mut out, 32); // HeaderSize
        out.extend_from_slice(&[0xFF, 0xFF]);
        le16(&mut out, e.kind);
        out.extend_from_slice(&[0xFF, 0xFF]);
        le16(&mut out, e.name);
        le32(&mut out, 0); // DataVersion
        le16(&mut out, e.flags);
        le16(&mut out, e.language);
        le32(&mut out, 0); // Version
        le32(&mut out, 0); // Characteristics
        out.extend_from_slice(&e.data);
        pad4(&mut out);
    }
    out
}

/// The `RT_VERSION` entry.
pub fn version_entry(data: Vec<u8>) -> Entry {
    Entry {
        kind: RT_VERSION,
        name: 1,
        flags: 0x0030, // MOVEABLE | PURE
        language: 0x0409,
        data,
    }
}

/// The icon resources for the `.ico` file bytes `ico`: one `RT_ICON` per
/// image (ids 1..=n, the DIB/PNG bytes as stored) and an `RT_GROUP_ICON` with
/// id 1, the lowest group id, which Explorer, the taskbar, Alt+Tab and Task
/// Manager use as the executable's icon. `None` for a malformed file.
pub fn icon_entries(ico: &[u8]) -> Option<Vec<Entry>> {
    let word = |at: usize| Some(u16::from_le_bytes(ico.get(at..at + 2)?.try_into().ok()?));
    let dword = |at: usize| Some(u32::from_le_bytes(ico.get(at..at + 4)?.try_into().ok()?));
    if word(0)? != 0 || word(2)? != 1 {
        return None;
    }
    let count = word(4)?;
    let mut group = Vec::new();
    le16(&mut group, 0);
    le16(&mut group, 1);
    le16(&mut group, count);
    let mut entries = Vec::new();
    for i in 0..count {
        let at = 6 + 16 * usize::from(i);
        let size = dword(at + 8)?;
        let offset = usize::try_from(dword(at + 12)?).ok()?;
        let image = ico.get(offset..offset.checked_add(usize::try_from(size).ok()?)?)?;
        // GRPICONDIRENTRY: the directory entry minus the offset, plus the id.
        group.extend_from_slice(ico.get(at..at + 12)?);
        le16(&mut group, i + 1);
        entries.push(Entry {
            kind: RT_ICON,
            name: i + 1,
            flags: 0x1030, // MOVEABLE | PURE | DISCARDABLE
            language: 0,
            data: image.to_vec(),
        });
    }
    entries.push(Entry {
        kind: RT_GROUP_ICON,
        name: 1,
        flags: 0x1030,
        language: 0,
        data: group,
    });
    Some(entries)
}

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return Ok(());
    }
    let part = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(0)
    };
    let parts = [
        part("CARGO_PKG_VERSION_MAJOR"),
        part("CARGO_PKG_VERSION_MINOR"),
        part("CARGO_PKG_VERSION_PATCH"),
        0,
    ];
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let out =
        PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default()).join("tokenmon-version.res");
    let ico_path = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("assets")
        .join("tokenmon.ico");
    println!("cargo:rerun-if-changed={}", ico_path.display());
    let ico = std::fs::read(&ico_path)?;
    let icons = icon_entries(&ico).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "assets/tokenmon.ico is malformed",
        )
    })?;
    let mut entries = icons;
    entries.push(version_entry(version_info(&version, parts)));
    std::fs::write(&out, res_file(&entries))?;
    println!("cargo:rustc-link-arg-bins={}", out.display());
    Ok(())
}
