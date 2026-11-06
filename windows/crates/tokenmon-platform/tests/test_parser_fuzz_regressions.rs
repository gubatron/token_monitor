//! Stable-toolchain replay of the `update_verify_pe` fuzz target: every corpus
//! seed, every truncation and bit flip of a valid PE, and a deterministic
//! random sweep must satisfy the target's invariants.
#![allow(clippy::unwrap_used)]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use tokenmon_platform::install::relaunch::parse_wait_for_pid;
use tokenmon_platform::install::verify::{is_pe_reader, zone_id_marks_internet};

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dir: PathBuf =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/update_verify_pe");
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

fn check(data: &[u8]) {
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
}

#[test]
fn test_corpus_seeds_hold_the_invariants() {
    let seeds = corpus();
    assert!(seeds.len() >= 6);
    for (_, data) in &seeds {
        check(data);
    }
    let by_name = |n: &str| seeds.iter().find(|(name, _)| name == n).unwrap().1.clone();
    assert!(is_pe_reader(Cursor::new(by_name("valid_pe"))).unwrap());
    assert!(!is_pe_reader(Cursor::new(by_name("mz_only"))).unwrap());
    assert!(zone_id_marks_internet(
        &String::from_utf8(by_name("zone_internet")).unwrap()
    ));
    assert_eq!(
        parse_wait_for_pid(
            String::from_utf8(by_name("wait_for_pid"))
                .unwrap()
                .split_whitespace()
        ),
        Some(42)
    );
}

/// An `e_lfanew` far past the end of the file is `Ok(false)` or an I/O error,
/// never a panic or a huge allocation.
#[test]
fn test_pe_offset_past_end_is_not_a_pe() {
    let mut pe = vec![0u8; 0x44];
    pe[..2].copy_from_slice(b"MZ");
    pe[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(!is_pe_reader(Cursor::new(pe)).unwrap_or(false));
}

#[test]
fn test_truncations_and_flips_of_a_valid_pe_never_panic() {
    let pe = corpus()
        .into_iter()
        .find(|(n, _)| n == "valid_pe")
        .unwrap()
        .1;
    for cut in 0..=pe.len() {
        check(&pe[..cut]);
    }
    for i in 0..pe.len() {
        let mut m = pe.clone();
        m[i] ^= 0xFF;
        check(&m);
    }
}

#[test]
fn test_random_sweep_holds_the_invariants() {
    let tokens: [&[u8]; 9] = [
        b"MZ",
        b"PE\0\0",
        b"--wait-for-pid",
        b"--wait-for-pid=",
        b" 42 ",
        b"0",
        b"ZoneId=3\n",
        b"[ZoneTransfer]\r\n",
        b"\xff\xfe",
    ];
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..20_000 {
        let mut data = Vec::new();
        for _ in 0..=(next() % 6) {
            data.extend_from_slice(tokens[(next() % tokens.len() as u64) as usize]);
            data.extend((0..next() % 70).map(|_| (next() & 0xFF) as u8));
        }
        check(&data);
    }
}
