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
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in entries {
        w.start_file(*name, SimpleFileOptions::default()).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap().into_inner()
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
