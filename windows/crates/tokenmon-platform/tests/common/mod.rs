//! Shared helpers for the install tests.
#![allow(dead_code, clippy::unwrap_used)]

use std::io::Write;

use tokenmon_core::update::sha256_hex;
use zip::write::SimpleFileOptions;

/// A PE32 image with a real `.rsrc` section holding a `VS_VERSIONINFO`
/// resource. `product` is the `ProductVersion` string and `fixed` the 4-part
/// file version; either may be omitted. `tag` is appended so contents differ.
pub fn pe_with_version(product: Option<&str>, fixed: Option<[u16; 4]>, tag: &str) -> Vec<u8> {
    const RAW: usize = 0x200;
    const VA: u32 = 0x1000;
    let blob = version_blob(product, fixed);
    // Resource tree: type dir, name dir, language dir, data entry, blob.
    let mut rsrc = Vec::new();
    let dir = |rsrc: &mut Vec<u8>, id: u32, next: u32| {
        rsrc.extend_from_slice(&[0u8; 12]);
        rsrc.extend_from_slice(&0u16.to_le_bytes()); // named entries
        rsrc.extend_from_slice(&1u16.to_le_bytes()); // id entries
        rsrc.extend_from_slice(&id.to_le_bytes());
        rsrc.extend_from_slice(&next.to_le_bytes());
    };
    dir(&mut rsrc, 16, 0x8000_0018);
    dir(&mut rsrc, 1, 0x8000_0030);
    dir(&mut rsrc, 0x409, 72);
    rsrc.extend_from_slice(&(VA + 88).to_le_bytes());
    rsrc.extend_from_slice(&(blob.len() as u32).to_le_bytes());
    rsrc.extend_from_slice(&[0u8; 8]);
    rsrc.extend_from_slice(&blob);
    let mut b = vec![0u8; 0x40];
    b[0] = b'M';
    b[1] = b'Z';
    b[0x3C] = 0x40;
    b.extend_from_slice(b"PE\0\0");
    let mut coff = [0u8; 20];
    coff[2..4].copy_from_slice(&1u16.to_le_bytes()); // sections
    coff[16..18].copy_from_slice(&224u16.to_le_bytes()); // optional header size
    b.extend_from_slice(&coff);
    let mut opt = vec![0u8; 224];
    opt[..2].copy_from_slice(&0x10Bu16.to_le_bytes());
    opt[92..96].copy_from_slice(&16u32.to_le_bytes());
    opt[96 + 16..96 + 20].copy_from_slice(&VA.to_le_bytes());
    opt[96 + 20..96 + 24].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
    b.extend_from_slice(&opt);
    let mut sec = [0u8; 40];
    sec[..6].copy_from_slice(b".rsrc\0");
    sec[8..12].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
    sec[12..16].copy_from_slice(&VA.to_le_bytes());
    sec[16..20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
    sec[20..24].copy_from_slice(&(RAW as u32).to_le_bytes());
    b.extend_from_slice(&sec);
    b.resize(RAW, 0);
    b.extend_from_slice(&rsrc);
    b.extend_from_slice(tag.as_bytes());
    b
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// One `VS_VERSIONINFO`-style node (`wLength, wValueLength, wType, key,
/// value, children`), 4-aligned.
fn ver_node(key: &str, text: bool, value_len: usize, value: &[u8], kids: &[u8]) -> Vec<u8> {
    let pad = |v: &mut Vec<u8>| v.resize(v.len().div_ceil(4) * 4, 0);
    let mut n = vec![0u8; 6];
    n.extend(utf16z(key));
    pad(&mut n);
    n.extend_from_slice(value);
    pad(&mut n);
    n.extend_from_slice(kids);
    let len = n.len() as u16;
    n[..2].copy_from_slice(&len.to_le_bytes());
    n[2..4].copy_from_slice(&(value_len as u16).to_le_bytes());
    n[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    n
}

fn version_blob(product: Option<&str>, fixed: Option<[u16; 4]>) -> Vec<u8> {
    let mut kids = Vec::new();
    if let Some(p) = product {
        let value = utf16z(p);
        let s = ver_node("ProductVersion", true, value.len() / 2, &value, &[]);
        let table = ver_node("040904B0", true, 0, &[], &s);
        kids = ver_node("StringFileInfo", true, 0, &[], &table);
    }
    let fixed_bytes = fixed.map_or_else(Vec::new, |[a, b, c, d]| {
        let mut f = vec![0u8; 52];
        f[..4].copy_from_slice(&0xFEEF_04BDu32.to_le_bytes());
        f[8..12].copy_from_slice(&((u32::from(a) << 16) | u32::from(b)).to_le_bytes());
        f[12..16].copy_from_slice(&((u32::from(c) << 16) | u32::from(d)).to_le_bytes());
        f
    });
    ver_node(
        "VS_VERSION_INFO",
        false,
        fixed_bytes.len(),
        &fixed_bytes,
        &kids,
    )
}

/// A PE that passes the structure check and declares version 1.6.0 (the
/// release the install tests serve), unless `tag` is `version=X.Y.Z`, which
/// declares that version instead. `tag` is also appended so contents are unique.
pub fn fake_pe(tag: &str) -> Vec<u8> {
    let version = tag.strip_prefix("version=").unwrap_or("1.6.0");
    pe_with_version(Some(version), None, tag)
}

/// A buffer with the OLE magic every MSI starts with.
pub fn fake_msi() -> Vec<u8> {
    let mut b = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    b.extend_from_slice(b"msi body");
    b
}

/// Builds a zip in memory from `(name, bytes)` pairs.
pub fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    // The zip writer normalises hostile names (`..`, `\\`, drive letters), so
    // write same-length placeholders and patch the real bytes into both the
    // local headers and the central directory afterwards.
    let placeholders: Vec<Vec<u8>> = entries
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let tag = format!("Q{i}Q");
            let mut p = tag.into_bytes();
            p.resize(name.len().max(p.len()), b'Q');
            p
        })
        .collect();
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for ((_, data), ph) in entries.iter().zip(&placeholders) {
        let ph = std::str::from_utf8(ph).unwrap();
        w.start_file(ph, SimpleFileOptions::default()).unwrap();
        w.write_all(data).unwrap();
    }
    let mut bytes = w.finish().unwrap().into_inner();
    for ((name, _), ph) in entries.iter().zip(&placeholders) {
        assert_eq!(name.len(), ph.len(), "name too short for placeholder");
        let mut from = 0;
        while let Some(pos) = bytes[from..]
            .windows(ph.len())
            .position(|w| w == ph.as_slice())
        {
            let at = from + pos;
            bytes[at..at + ph.len()].copy_from_slice(name.as_bytes());
            from = at + ph.len();
        }
    }
    bytes
}

/// A zip containing a symbolic link entry.
pub fn make_zip_with_symlink() -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("TokenMon.exe", SimpleFileOptions::default())
        .unwrap();
    w.write_all(&fake_pe("new")).unwrap();
    w.add_symlink("link", "/etc/passwd", SimpleFileOptions::default())
        .unwrap();
    w.finish().unwrap().into_inner()
}

pub fn sha(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}
