//! Startup helpers the app calls: `--wait-for-pid` handling and leftover cleanup.

use tokenmon_platform::install::{handle_startup_args, parse_wait_for_pid};

/// `handle_startup_args` removes leftovers and reports the waited pid.
#[test]
fn test_handle_startup_args_cleans_and_reports_pid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join("TokenMon.exe");
    std::fs::write(&exe, b"x").expect("write");
    let stale = dir.path().join(".TokenMon-update-abc");
    std::fs::write(&stale, b"x").expect("write");
    let pid = handle_startup_args(["TokenMon.exe", "--wait-for-pid", "1234567"], &exe);
    assert_eq!(pid, Some(1_234_567));
    assert!(!stale.exists());
    assert!(exe.exists());
}

/// Without the flag nothing is waited for, but cleanup still runs.
#[test]
fn test_handle_startup_args_without_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join("TokenMon.exe");
    std::fs::write(&exe, b"x").expect("write");
    assert_eq!(handle_startup_args(["TokenMon.exe"], &exe), None);
}

/// Malformed values are ignored rather than trusted.
#[test]
fn test_parse_wait_for_pid_rejects_garbage() {
    assert_eq!(parse_wait_for_pid(["x", "--wait-for-pid"]), None);
    assert_eq!(parse_wait_for_pid(["x", "--wait-for-pid", "-1"]), None);
    assert_eq!(
        parse_wait_for_pid(["x", "--wait-for-pid", "99999999999"]),
        None
    );
}
