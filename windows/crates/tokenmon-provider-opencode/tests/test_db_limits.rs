//! Untrusted-database limits (issue #49): views are rejected, the query time
//! budget and the row cap fail closed, and the connection is hardened.
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use tokenmon_provider_opencode::db::{
    DbError, ScanCache, load_scan, open_read_only, open_read_only_with_budget,
    read_assistant_message_rows, read_assistant_message_rows_capped, scan_start,
};

const SCHEMA: &str = "CREATE TABLE session (id, time_created, time_archived, model);
CREATE TABLE message (id, session_id, time_created, time_updated, data);";

fn make_db(dir: &Path, extra: &str) -> std::path::PathBuf {
    let path = dir.join("opencode.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn.execute_batch(extra).unwrap();
    path
}

fn insert_messages(conn: &Connection, n: i64) {
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO message VALUES (?1, 's', 1785580000000, 0, \
                 '{\"role\":\"assistant\",\"providerID\":\"p\",\"modelID\":\"m\",\"cost\":0.1,\"tokens\":{\"input\":1,\"output\":2,\"cache\":{\"read\":3,\"write\":4}}}')",
            )
            .unwrap();
        for i in 0..n {
            stmt.execute([i]).unwrap();
        }
    }
    tx.commit().unwrap();
}

/// A `message` view could hide an expensive definition: refuse it.
#[test]
fn a_message_view_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (id, time_created, time_archived, model);
         CREATE TABLE raw_message (id, session_id, time_created, time_updated, data);
         CREATE VIEW message AS SELECT * FROM raw_message;",
    )
    .unwrap();
    drop(conn);
    assert!(matches!(open_read_only(&path), Err(DbError::OpenFailed(_))));
    let now = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    assert!(load_scan(&path, scan_start(now), &ScanCache::default()).is_err());
}

/// An expensive query is interrupted by the time budget instead of pinning
/// the polling thread.
#[test]
fn an_expired_time_budget_aborts_the_query() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_db(dir.path(), "");
    insert_messages(&Connection::open(&path).unwrap(), 20_000);
    let conn = open_read_only_with_budget(&path, Duration::ZERO).unwrap();
    let err = read_assistant_message_rows(&conn, 0).unwrap_err();
    assert!(matches!(err, DbError::QueryFailed(_)), "{err:?}");
    // The normal budget reads the same database fine.
    let conn = open_read_only(&path).unwrap();
    assert_eq!(read_assistant_message_rows(&conn, 0).unwrap().len(), 20_000);
}

/// More rows than the cap is an error, never a silently truncated scan.
#[test]
fn exceeding_the_row_cap_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_db(dir.path(), "");
    insert_messages(&Connection::open(&path).unwrap(), 10);
    let conn = open_read_only(&path).unwrap();
    assert_eq!(
        read_assistant_message_rows_capped(&conn, 0, 10)
            .unwrap()
            .len(),
        10
    );
    let err = read_assistant_message_rows_capped(&conn, 0, 9).unwrap_err();
    assert!(matches!(err, DbError::QueryFailed(m) if m.contains("more than 9")));
}

/// The connection cannot write, even through a statement that would.
#[test]
fn the_connection_is_read_only_and_query_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = make_db(dir.path(), "");
    let conn = open_read_only(&path).unwrap();
    assert!(
        conn.execute_batch("INSERT INTO session VALUES (1,2,3,4)")
            .is_err()
    );
    assert!(conn.execute_batch("CREATE TABLE x (a)").is_err());
    let q: i64 = conn
        .query_row("PRAGMA query_only", [], |r| r.get(0))
        .unwrap();
    assert_eq!(q, 1);
    let t: i64 = conn
        .query_row("PRAGMA trusted_schema", [], |r| r.get(0))
        .unwrap();
    assert_eq!(t, 0);
}
