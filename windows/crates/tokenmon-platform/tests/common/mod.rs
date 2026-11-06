//! Shared helpers for the install tests.
#![allow(dead_code, clippy::unwrap_used)]

use std::io::Write;

use tokenmon_core::update::sha256_hex;
use zip::write::SimpleFileOptions;

/// A minimal buffer that passes the PE structure check; `tag` makes contents unique.
pub fn fake_pe(tag: &str) -> Vec<u8> {
    let mut b = vec![0u8; 0x40];
    b[0] = b'M';
    b[1] = b'Z';
    b[0x3C] = 0x40;
    b.extend_from_slice(b"PE\0\0");
    b.extend_from_slice(tag.as_bytes());
    b
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
