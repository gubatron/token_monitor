//! The rolling file log and the panic hook (#66).

use std::fs;
use std::sync::Arc;

use tokenmon_app::logging::{
    LOG_FILE_NAME, RollingFile, describe_panic, file_subscriber, install_panic_hook, log_dir,
};

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Events land in the file, with no ANSI colour codes.
#[test]
fn test_events_are_written_to_the_log_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = Arc::new(RollingFile::open(dir.path(), 1 << 20, 3).expect("open"));
    tracing::subscriber::with_default(file_subscriber(&file), || {
        tracing::warn!("tray icon failed");
    });
    let text = fs::read_to_string(dir.path().join(LOG_FILE_NAME)).expect("read");
    assert!(text.contains("tray icon failed"), "{text}");
    assert!(!text.contains('\u{1b}'));
}

/// The folder never grows past `max_files` files of about `max_file_bytes`.
#[test]
fn test_log_is_bounded_in_size_and_count() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = Arc::new(RollingFile::open(dir.path(), 200, 3).expect("open"));
    tracing::subscriber::with_default(file_subscriber(&file), || {
        for i in 0..200 {
            tracing::warn!("line number {i} padding padding padding");
        }
    });
    assert_eq!(
        names(dir.path()),
        vec!["tokenmon.log", "tokenmon.log.1", "tokenmon.log.2"]
    );
    for name in names(dir.path()) {
        let len = fs::metadata(dir.path().join(name)).expect("meta").len();
        assert!(len < 400, "{len}");
    }
    let newest = fs::read_to_string(dir.path().join(LOG_FILE_NAME)).expect("read");
    assert!(newest.contains("line number 199"));
}

/// A single file keeps no history but still logs.
#[test]
fn test_single_file_log_keeps_only_the_active_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = Arc::new(RollingFile::open(dir.path(), 100, 1).expect("open"));
    tracing::subscriber::with_default(file_subscriber(&file), || {
        for i in 0..20 {
            tracing::warn!("line {i} padding padding padding padding");
        }
    });
    assert_eq!(names(dir.path()), vec!["tokenmon.log"]);
}

/// Reopening appends to the existing file instead of truncating it.
#[test]
fn test_reopen_appends() {
    let dir = tempfile::tempdir().expect("tempdir");
    for word in ["first", "second"] {
        let file = Arc::new(RollingFile::open(dir.path(), 1 << 20, 2).expect("open"));
        tracing::subscriber::with_default(file_subscriber(&file), || tracing::warn!("{word}"));
    }
    let text = fs::read_to_string(dir.path().join(LOG_FILE_NAME)).expect("read");
    assert!(text.contains("first") && text.contains("second"));
}

/// Both payload kinds and the location are described.
#[test]
fn test_describe_panic_covers_str_string_and_other_payloads() {
    let s = std::panic::catch_unwind(|| panic!("static")).unwrap_err();
    assert_eq!(describe_panic(s.as_ref(), None), "panic: static");
    let owned = std::panic::catch_unwind(|| panic!("{}", String::from("owned"))).unwrap_err();
    assert_eq!(describe_panic(owned.as_ref(), None), "panic: owned");
    let other = std::panic::catch_unwind(|| std::panic::panic_any(7_u8)).unwrap_err();
    assert!(describe_panic(other.as_ref(), None).contains("non-string"));
    let here = std::panic::Location::caller();
    assert!(describe_panic(s.as_ref(), Some(here)).contains("logging.rs"));
}

/// A panic reaches the log file through the hook (release builds abort right
/// after the hook returns, so the line must already be on disk).
#[test]
fn test_panic_hook_logs_the_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = Arc::new(RollingFile::open(&log_dir(dir.path()), 1 << 20, 2).expect("open"));
    install_panic_hook();
    tracing::subscriber::with_default(file_subscriber(&file), || {
        let _ = std::panic::catch_unwind(|| panic!("poller exploded"));
    });
    let text = fs::read_to_string(log_dir(dir.path()).join(LOG_FILE_NAME)).expect("read");
    assert!(text.contains("poller exploded"), "{text}");
    assert!(text.contains("logging.rs"), "{text}");
}
