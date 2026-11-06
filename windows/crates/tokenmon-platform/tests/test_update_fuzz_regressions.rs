//! Stable-toolchain replay of the `update_archive` fuzz target: every corpus
//! seed plus random bytes must extract without panicking, stay inside the
//! destination and stay under the byte cap.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use proptest_free::check_archive;

mod proptest_free {
    use std::path::Path;

    use tokenmon_platform::install::archive::extract_zip;

    pub const CAP: u64 = 1 << 20;

    pub fn dir_size(dir: &Path) -> u64 {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries
                .flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
    }

    pub fn check_archive(data: &[u8]) {
        let root = tempfile::tempdir().unwrap();
        let zip = root.path().join("in.zip");
        std::fs::write(&zip, data).unwrap();
        let dest = root.path().join("out");
        let _ = extract_zip(&zip, &dest, CAP);
        assert!(dir_size(&dest) <= CAP);
        let stray = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name() != "in.zip" && e.file_name() != "out")
            .count();
        assert_eq!(stray, 0, "extraction escaped its destination");
    }
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/update_archive")
}

/// Every committed seed is handled safely.
#[test]
fn test_archive_corpus_seeds_are_handled_safely() {
    let mut seen = 0;
    for entry in std::fs::read_dir(corpus_dir()).unwrap().flatten() {
        check_archive(&std::fs::read(entry.path()).unwrap());
        seen += 1;
    }
    assert!(seen >= 2);
}

/// Truncated and bit-flipped variants of a valid zip never panic or escape.
#[test]
fn test_archive_mutations_are_handled_safely() {
    let ok = std::fs::read(corpus_dir().join("ok.zip")).unwrap();
    for cut in 0..ok.len() {
        check_archive(&ok[..cut]);
    }
    for i in 0..ok.len() {
        let mut m = ok.clone();
        m[i] ^= 0xFF;
        check_archive(&m);
    }
}
