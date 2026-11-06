#![no_main]
//! The install-time parsers of attacker-influenced input never panic:
//! `is_pe_reader` (downloaded file header), `zone_id_marks_internet`
//! (`Zone.Identifier` text) and `parse_wait_for_pid` (command line).
//!
//! Invariants: a PE verdict implies `MZ` and a `PE\0\0` signature at
//! `e_lfanew`; an Internet-zone verdict implies a `ZoneId=` line; a parsed
//! pid is non-zero and came from a `--wait-for-pid` argument.

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::install::relaunch::parse_wait_for_pid;
use tokenmon_platform::install::verify::{is_pe_reader, zone_id_marks_internet};

fuzz_target!(|data: &[u8]| {
    if let Ok(true) = is_pe_reader(Cursor::new(data)) {
        assert!(data.starts_with(b"MZ") && data.len() >= 0x40);
        let off = u32::from_le_bytes([data[0x3C], data[0x3D], data[0x3E], data[0x3F]]) as usize;
        assert_eq!(data.get(off..off + 4), Some(&b"PE\0\0"[..]));
    }
    let text = String::from_utf8_lossy(data);
    if zone_id_marks_internet(&text) {
        assert!(text.contains("ZoneId="));
    }
    let args: Vec<&str> = text.split_whitespace().collect();
    if let Some(pid) = parse_wait_for_pid(&args) {
        assert_ne!(pid, 0);
        assert!(text.contains("--wait-for-pid"));
    }
});
