//! Stable-toolchain regression tests for the `opencode_db` fuzz target:
//! crafted `opencode.db` files must be rejected with a typed error or scanned
//! to sane rows, never panic. Replays the committed fuzz corpus too.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;

use chrono::{TimeZone, Utc};
use common::{Fixture, Msg};
use tokenmon_provider_opencode::db::{MAX_ROW_MS, ScanCache, load_scan, scan_start};
use tokenmon_provider_opencode::stats::{build_day_hourly_usage, snapshot};

/// What the fuzz target does with one file: scan it and build everything from it.
fn exercise(path: &Path) {
    let now = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let Ok(scan) = load_scan(path, scan_start(now), &ScanCache::default()) else {
        return;
    };
    for row in &scan.rows {
        assert!(!row.provider_id.is_empty() && !row.model_id.is_empty());
        assert!(row.date.timestamp_millis() <= MAX_ROW_MS);
        assert!(row.cost_usd.is_finite());
    }
    let snap = snapshot(&scan, now);
    assert!(snap.monthly_used_percent().is_finite());
    let utc = chrono::FixedOffset::east_opt(0).unwrap();
    let cal = tokenmon_core::usage::Calendar::fixed(utc);
    let _ = build_day_hourly_usage(&scan.rows, now, &cal, 8);
}

fn exercise_bytes(bytes: &[u8]) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    std::fs::write(&path, bytes).unwrap();
    exercise(&path);
}

/// Every file of the committed fuzz corpus scans without panicking.
#[test]
fn test_regression_fuzz_corpus_replays_cleanly() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/opencode_db");
    let entries: Vec<_> = std::fs::read_dir(corpus).unwrap().collect();
    assert!(!entries.is_empty());
    for entry in entries {
        exercise_bytes(&std::fs::read(entry.unwrap().path()).unwrap());
    }
}

/// Empty, non-SQLite and header-only files are rejected cleanly.
#[test]
fn test_regression_garbage_files_are_rejected() {
    exercise_bytes(b"");
    exercise_bytes(b"not a database at all");
    exercise_bytes(&[0xFF; 4096]);
    let mut header = b"SQLite format 3\0".to_vec();
    header.extend_from_slice(&[0; 84]);
    exercise_bytes(&header);
}

/// A real database with bytes flipped at many offsets, and truncated at page
/// fractions, never panics the scanner.
#[test]
fn test_regression_corrupted_real_database_never_panics() {
    let mut f = Fixture::new();
    f.insert(
        "s",
        Msg::go("s", 1_785_580_000, "kimi-k2.5", 1.5).tokens([1, 2, 3, 4]),
        false,
    );
    f.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").ok();
    let original = std::fs::read(&f.path).unwrap();
    for offset in (0..original.len()).step_by(97) {
        let mut copy = original.clone();
        copy[offset] ^= 0xA5;
        exercise_bytes(&copy);
    }
    for len in (0..original.len()).step_by(512) {
        exercise_bytes(&original[..len]);
    }
}

/// A schema with the right tables but wrong column types is coerced or skipped.
#[test]
fn test_regression_wrong_column_types_are_coerced_or_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (id, time_created, time_archived, model);
         CREATE TABLE message (id, session_id, time_created, time_updated, data);
         INSERT INTO session VALUES (x'00', 'soon', 'never', 42);
         INSERT INTO message VALUES (1, 2.5, 'x', NULL, x'7b7d');
         INSERT INTO message VALUES (2, NULL, 9223372036854775807, 0, '[1,2');",
    )
    .unwrap();
    drop(conn);
    exercise(&path);
}
