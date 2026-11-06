//! Gives `TokenMon.exe` a `VS_VERSIONINFO` resource (#77).
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

/// A `.res` file: the mandatory empty first entry, then `RT_VERSION` id 1.
fn res_file(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u32.to_le_bytes()); // DataSize
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize
    out.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    out.extend_from_slice(&[0u8; 16]);

    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF, 16, 0]); // type RT_VERSION
    out.extend_from_slice(&[0xFF, 0xFF, 1, 0]); // name 1
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&0x0030u16.to_le_bytes()); // MOVEABLE | PURE
    out.extend_from_slice(&0x0409u16.to_le_bytes()); // language
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    pad4(&mut out);
    out
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
    std::fs::write(&out, res_file(&version_info(&version, parts)))?;
    println!("cargo:rustc-link-arg-bins={}", out.display());
    Ok(())
}
