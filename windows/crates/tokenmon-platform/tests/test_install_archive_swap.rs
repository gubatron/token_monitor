//! Archive extraction limits and the staged exe swap.
#![allow(clippy::unwrap_used)]

mod common;

use std::fs;

use common::{fake_pe, make_zip, make_zip_with_symlink};
use tokenmon_core::update::InstallError;
use tokenmon_platform::install::archive::{extract_zip, locate_exe};
use tokenmon_platform::install::swap::{
    can_replace_running_exe, cleanup_leftovers, commit_swap, stage_next_to,
};

fn archive_msg(e: InstallError) -> String {
    match e {
        InstallError::Archive(m) => m,
        other => panic!("expected archive error, got {other:?}"),
    }
}

/// A normal zip extracts and exposes its top-level exe.
#[test]
fn test_extracts_and_locates_exe() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("a.zip");
    fs::write(
        &zip,
        make_zip(&[("TokenMon.exe", &fake_pe("x")), ("docs/readme.txt", b"hi")]),
    )
    .unwrap();
    let out = dir.path().join("out");
    extract_zip(&zip, &out, 1 << 20).unwrap();
    assert!(locate_exe(&out).unwrap().is_file());
    assert!(out.join("docs/readme.txt").is_file());
}

/// An exe nested below the top level is not accepted.
#[test]
fn test_nested_exe_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("a.zip");
    fs::write(&zip, make_zip(&[("TokenMon/TokenMon.exe", &fake_pe("x"))])).unwrap();
    let out = dir.path().join("out");
    extract_zip(&zip, &out, 1 << 20).unwrap();
    assert!(archive_msg(locate_exe(&out).unwrap_err()).contains("top level"));
}

/// Symlink entries are refused.
#[test]
fn test_symlink_entry_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("a.zip");
    fs::write(&zip, make_zip_with_symlink()).unwrap();
    let e = extract_zip(&zip, &dir.path().join("out"), 1 << 20).unwrap_err();
    assert_eq!(archive_msg(e), "The release zip contains a symbolic link.");
}

/// Path traversal and drive/stream names are refused and nothing escapes.
#[test]
fn test_unsafe_paths_are_refused() {
    for name in [
        "../evil.txt",
        "a/../../evil.txt",
        "/abs.txt",
        "C:\\evil.txt",
        "x.exe:stream",
        "a\\b.txt",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("a.zip");
        fs::write(&zip, make_zip(&[(name, b"x")])).unwrap();
        let out = dir.path().join("out");
        let e = extract_zip(&zip, &out, 1 << 20).unwrap_err();
        assert!(archive_msg(e).contains("unsafe path"), "{name}");
        assert!(!dir.path().join("evil.txt").exists());
    }
}

/// Declared and actual size are both capped.
#[test]
fn test_size_cap_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("a.zip");
    fs::write(
        &zip,
        make_zip(&[("a.bin", &[0u8; 5000]), ("b.bin", &[0u8; 5000])]),
    )
    .unwrap();
    let e = extract_zip(&zip, &dir.path().join("out"), 9_999).unwrap_err();
    assert_eq!(archive_msg(e), "The release zip is unexpectedly large.");
    extract_zip(&zip, &dir.path().join("out2"), 10_000).unwrap();
}

/// Garbage is not a zip.
#[test]
fn test_garbage_is_not_a_zip() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("a.zip");
    fs::write(&zip, b"not a zip").unwrap();
    let e = extract_zip(&zip, &dir.path().join("out"), 1 << 20).unwrap_err();
    assert_eq!(archive_msg(e), "The release zip could not be opened.");
}

/// Swap puts the new bytes in place, keeps the old one aside, and cleanup removes it.
#[test]
fn test_stage_swap_and_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let running = dir.path().join("TokenMon.exe");
    let new = dir.path().join("new.exe");
    fs::write(&running, b"old").unwrap();
    fs::write(&new, b"new").unwrap();
    assert!(can_replace_running_exe(&running));
    let staged = stage_next_to(&new, &running).unwrap();
    let aside = commit_swap(staged, &running).unwrap();
    assert_eq!(fs::read(&running).unwrap(), b"new");
    assert_eq!(fs::read(&aside).unwrap(), b"old");
    assert!(
        aside
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("TokenMon.old-")
    );
    fs::write(dir.path().join(".TokenMon-update-abc.exe"), b"half").unwrap();
    fs::write(dir.path().join("other.exe"), b"keep").unwrap();
    cleanup_leftovers(&running);
    assert!(!aside.exists());
    assert!(!dir.path().join(".TokenMon-update-abc.exe").exists());
    assert!(
        dir.path().join("other.exe").exists(),
        "unrelated files stay"
    );
    assert_eq!(fs::read(&running).unwrap(), b"new");
}

/// A failed commit restores the original and leaves no siblings.
#[test]
fn test_failed_commit_restores_original() {
    let dir = tempfile::tempdir().unwrap();
    let running = dir.path().join("TokenMon.exe");
    let new = dir.path().join("new.exe");
    fs::write(&running, b"old").unwrap();
    fs::write(&new, b"new").unwrap();
    let staged = stage_next_to(&new, &running).unwrap();
    fs::remove_file(staged.path()).unwrap(); // sabotage the rename
    assert!(commit_swap(staged, &running).is_err());
    assert_eq!(fs::read(&running).unwrap(), b"old");
    assert_eq!(
        fs::read_dir(dir.path()).unwrap().count(),
        2,
        "only exe and new.exe remain"
    );
}

/// Dropping a staged exe removes it.
#[test]
fn test_dropped_staged_exe_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let running = dir.path().join("TokenMon.exe");
    let new = dir.path().join("new.exe");
    fs::write(&running, b"old").unwrap();
    fs::write(&new, b"new").unwrap();
    let staged = stage_next_to(&new, &running).unwrap();
    let p = staged.path().to_path_buf();
    assert!(p.exists());
    drop(staged);
    assert!(!p.exists());
}

/// Only existing `.exe` files in writable folders can be replaced.
#[test]
fn test_can_replace_checks_the_given_path() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("TokenMon.exe");
    assert!(!can_replace_running_exe(&exe), "missing file");
    fs::write(&exe, b"x").unwrap();
    assert!(can_replace_running_exe(&exe));
    let not_exe = dir.path().join("TokenMon.bin");
    fs::write(&not_exe, b"x").unwrap();
    assert!(!can_replace_running_exe(&not_exe));
}

/// `Zone.Identifier` text with ZoneId 3 or 4 marks an Internet download.
#[test]
fn test_zone_identifier_parsing() {
    use tokenmon_platform::install::verify::zone_id_marks_internet as z;
    assert!(z("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet"));
    assert!(z("ZoneId=4"));
    assert!(!z("[ZoneTransfer]\r\nZoneId=0"));
    assert!(!z("[ZoneTransfer]\r\nZoneId=x"));
    assert!(!z(""));
}
