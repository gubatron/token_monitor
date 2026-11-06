#![no_main]
//! Zip extraction never panics, never writes outside its destination and
//! never exceeds the byte cap.

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::install::archive::extract_zip;

const CAP: u64 = 1 << 20;

fn dir_size(dir: &Path) -> u64 {
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

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let root = tempfile::tempdir().expect("tempdir");
    let zip = root.path().join("in.zip");
    std::fs::write(&zip, data).expect("write input");
    let dest = root.path().join("out");
    let _ = extract_zip(&zip, &dest, CAP);
    assert!(dir_size(&dest) <= CAP);
    let stray: Vec<_> = std::fs::read_dir(root.path())
        .expect("read root")
        .flatten()
        .map(|e| e.file_name())
        .filter(|n| n != "in.zip" && n != "out")
        .collect();
    assert!(stray.is_empty(), "escaped destination: {stray:?}");
});
