//! The portable `VS_VERSIONINFO` reader (#77): real resource layouts parse,
//! hostile or truncated images yield `None` and never panic.
#![allow(clippy::unwrap_used)]

mod common;

use std::io::Cursor;

use common::pe_with_version;
use tokenmon_platform::install::read_pe_version;

fn read(bytes: Vec<u8>) -> Option<String> {
    read_pe_version(Cursor::new(bytes)).map(|v| v.to_string())
}

#[test]
fn test_product_version_string_wins_and_keeps_prerelease() {
    let pe = pe_with_version(Some("1.8.0-beta.2"), Some([1, 7, 0, 0]), "");
    assert_eq!(read(pe).as_deref(), Some("1.8.0-beta.2"));
}

#[test]
fn test_fixed_file_version_is_the_fallback() {
    let pe = pe_with_version(None, Some([1, 6, 3, 0]), "");
    let v = read_pe_version(Cursor::new(pe)).unwrap();
    assert_eq!(
        v,
        tokenmon_core::update::AppVersion::parse("1.6.3").unwrap()
    );
}

#[test]
fn test_no_resource_or_no_version_is_none() {
    assert!(read(pe_with_version(None, None, "")).is_none());
    assert!(read(Vec::new()).is_none());
    assert!(read(b"MZ".to_vec()).is_none());
    // A PE header with no sections at all.
    let mut bare = vec![0u8; 0x40];
    bare[..2].copy_from_slice(b"MZ");
    bare[0x3C] = 0x40;
    bare.extend_from_slice(b"PE\0\0");
    assert!(read(bare).is_none());
}

/// Every truncation of a valid image is total.
#[test]
fn test_every_truncation_is_total() {
    let pe = pe_with_version(Some("1.6.0"), Some([1, 6, 0, 0]), "tail");
    for n in 0..pe.len() {
        let _ = read(pe[..n].to_vec());
    }
}

/// Every single-byte corruption of a valid image is total.
#[test]
fn test_every_byte_flip_is_total() {
    let pe = pe_with_version(Some("1.6.0"), Some([1, 6, 0, 0]), "");
    for i in 0..pe.len() {
        for v in [0x00, 0xFF, 0x80] {
            let mut m = pe.clone();
            m[i] = v;
            let _ = read(m);
        }
    }
}

/// A resource directory that points back at itself terminates.
#[test]
fn test_self_referencing_resource_directory_terminates() {
    let mut pe = pe_with_version(Some("1.6.0"), None, "");
    // The type directory entry (at 0x200 + 16) points to its own directory.
    pe[0x200 + 20..0x200 + 24].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    assert!(read(pe).is_none());
}
