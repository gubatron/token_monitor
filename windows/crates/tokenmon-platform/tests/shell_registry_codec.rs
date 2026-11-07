//! The `REG_SZ` byte format used by the Win32 registry adapter
//! (`shell/windows/registry.rs`, excluded from coverage), tested on every OS.

use tokenmon_platform::shell::registry::{decode_reg_sz, encode_reg_sz};

/// Encoding is UTF-16LE with the terminating NUL `RegSetValueExW` needs.
#[test]
fn test_encode_reg_sz_is_utf16le_with_terminator() {
    assert_eq!(encode_reg_sz("A"), vec![0x41, 0x00, 0x00, 0x00]);
    assert_eq!(encode_reg_sz(""), vec![0x00, 0x00]);
    // U+00E9 and a supplementary-plane character (surrogate pair).
    assert_eq!(
        encode_reg_sz("é😀"),
        vec![0xE9, 0x00, 0x3D, 0xD8, 0x00, 0xDE, 0x00, 0x00]
    );
}

/// Whatever is written reads back unchanged.
#[test]
fn test_reg_sz_round_trips() {
    for value in [
        "",
        r#""C:\Program Files\TokenMon\TokenMon.exe" --background"#,
        "Ünïcödé 😀 text",
    ] {
        assert_eq!(decode_reg_sz(&encode_reg_sz(value)), value);
    }
}

/// Data is cut at the first NUL; a missing terminator or an odd trailing byte
/// (both possible in values other programs wrote) is tolerated.
#[test]
fn test_decode_reg_sz_tolerates_malformed_data() {
    assert_eq!(decode_reg_sz(&[0x41, 0x00, 0x00, 0x00, 0x42, 0x00]), "A");
    assert_eq!(decode_reg_sz(&[0x41, 0x00, 0x42, 0x00]), "AB");
    assert_eq!(decode_reg_sz(&[0x41, 0x00, 0x42]), "A");
    assert_eq!(decode_reg_sz(&[]), "");
    // An unpaired high surrogate becomes U+FFFD instead of failing.
    assert_eq!(decode_reg_sz(&[0x3D, 0xD8, 0x41, 0x00]), "\u{FFFD}A");
}
