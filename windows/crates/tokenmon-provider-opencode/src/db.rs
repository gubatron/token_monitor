//! Read-only access to `opencode.db` (spec 03 sections 3.3 and 3.4).
//!
//! The database belongs to OpenCode, which writes it (WAL) while TokenMon
//! reads. This module therefore never creates or modifies the file: it opens
//! with `SQLITE_OPEN_READ_ONLY` only, uses a plain path, and sets a busy
//! timeout so a writer holding a lock delays the read instead of failing it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use thiserror::Error;

use crate::pricing::{Billable, Tokens, billable_cost_usd};

/// Days of history one scan covers.
pub const SCAN_LOOKBACK_DAYS: i64 = 62;

/// Busy timeout for the read connection.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Why the local database could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DbError {
    /// No `opencode.db` at the probed path.
    #[error("OpenCode usage database not found at {0}. Install and use OpenCode to track usage.")]
    Missing(String),
    /// SQLite refused to open the file.
    #[error("Could not open the OpenCode usage database: {0}")]
    OpenFailed(String),
    /// A statement failed, including BUSY past the timeout and corruption.
    #[error("Could not read OpenCode usage: {0}")]
    QueryFailed(String),
}

/// Environment values used to find the data directory. Injected so path
/// logic is testable on every platform.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataDirEnv {
    /// `XDG_DATA_HOME`.
    pub xdg_data_home: Option<PathBuf>,
    /// `%USERPROFILE%` (Windows home).
    pub user_profile: Option<PathBuf>,
    /// `$HOME` (macOS and Linux home).
    pub home: Option<PathBuf>,
    /// `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
}

impl DataDirEnv {
    /// Reads the real process environment. Empty values count as unset.
    pub fn from_process() -> Self {
        let get = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Self {
            xdg_data_home: get("XDG_DATA_HOME"),
            user_profile: get("USERPROFILE"),
            home: get("HOME"),
            local_app_data: get("LOCALAPPDATA"),
        }
    }
}

/// Candidate `opencode.db` paths in probe order: `XDG_DATA_HOME`, then
/// `<profile>/.local/share/opencode`, then the same under `$HOME`, then
/// `%LOCALAPPDATA%\opencode` as a last resort. The real Windows location
/// still needs confirming on a machine with OpenCode installed.
pub fn candidate_db_paths(env: &DataDirEnv) -> Vec<PathBuf> {
    let share = |base: &Path| base.join(".local").join("share").join("opencode");
    let raw = [
        env.xdg_data_home.as_ref().map(|p| p.join("opencode")),
        env.user_profile.as_deref().map(share),
        env.home.as_deref().map(share),
        env.local_app_data.as_ref().map(|p| p.join("opencode")),
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in raw.into_iter().flatten() {
        let p = dir.join("opencode.db");
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// First candidate for which `exists` is true; else the first candidate (used
/// for the "not found at" message); `None` when no environment is known.
pub fn locate_db(env: &DataDirEnv, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let candidates = candidate_db_paths(env);
    candidates
        .iter()
        .find(|p| exists(p))
        .or_else(|| candidates.first())
        .cloned()
}

/// One assistant message with its recorded cost and token counts.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    /// Message creation time.
    pub date: DateTime<Utc>,
    /// Session id, trimmed; empty when NULL.
    pub session_id: String,
    /// Provider id, trimmed, never empty.
    pub provider_id: String,
    /// Model id, trimmed, never empty.
    pub model_id: String,
    /// Cost OpenCode recorded, USD (0 when NULL or not a finite number).
    pub cost_usd: f64,
    /// Token counts.
    pub tokens: Tokens,
}

impl MessageRow {
    /// Plan cost for this row, see [`billable_cost_usd`].
    pub fn billable(&self) -> Billable {
        billable_cost_usd(&self.provider_id, &self.model_id, self.cost_usd, self.tokens)
    }
}

/// Result of one database scan.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scan {
    /// Earliest non-archived Go session: the subscription anchor.
    pub subscribed_at: Option<DateTime<Utc>>,
    /// Assistant messages since the scan start.
    pub rows: Vec<MessageRow>,
}

/// SQLite-style integer coercion of a column.
fn coerce_i64(v: ValueRef<'_>) -> i64 {
    match v {
        ValueRef::Integer(i) => i,
        ValueRef::Real(f) => f as i64,
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map_or(0, |f| f as i64),
        ValueRef::Null | ValueRef::Blob(_) => 0,
    }
}

/// SQLite-style real coercion; non-finite results become 0.
fn coerce_f64(v: ValueRef<'_>) -> f64 {
    let f = match v {
        ValueRef::Integer(i) => i as f64,
        ValueRef::Real(f) => f,
        ValueRef::Text(t) => std::str::from_utf8(t)
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0),
        ValueRef::Null | ValueRef::Blob(_) => 0.0,
    };
    if f.is_finite() { f } else { 0.0 }
}

fn coerce_text(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Text(t) => String::from_utf8_lossy(t).trim().to_owned(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Null | ValueRef::Blob(_) => String::new(),
    }
}

fn query_err(e: &rusqlite::Error) -> DbError {
    DbError::QueryFailed(e.to_string())
}

/// Opens `path` strictly read-only. Never creates the file.
///
/// # Errors
/// [`DbError::Missing`] when the file is absent, [`DbError::OpenFailed`]
/// otherwise.
pub fn open_read_only(path: &Path) -> Result<Connection, DbError> {
    if !path.is_file() {
        return Err(DbError::Missing(path.display().to_string()));
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| DbError::OpenFailed(e.to_string()))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(|e| DbError::OpenFailed(e.to_string()))?;
    Ok(conn)
}

const ANCHOR_SQL: &str = "SELECT MIN(time_created) FROM session \
WHERE time_archived IS NULL \
AND lower(CASE WHEN json_valid(model) THEN json_extract(model, '$.providerID') END) = 'opencode-go'";

const ROWS_SQL: &str = "SELECT time_created, session_id, \
json_extract(data, '$.providerID'), json_extract(data, '$.modelID'), \
json_extract(data, '$.cost'), \
json_extract(data, '$.tokens.input'), json_extract(data, '$.tokens.output'), \
json_extract(data, '$.tokens.cache.read'), json_extract(data, '$.tokens.cache.write') \
FROM message \
WHERE time_created >= ?1 \
AND (CASE WHEN json_valid(data) THEN json_extract(data, '$.role') END) = 'assistant'";

/// Query 1: the earliest non-archived Go session, as the subscription anchor.
///
/// # Errors
/// [`DbError::QueryFailed`].
pub fn earliest_go_session_date(conn: &Connection) -> Result<Option<DateTime<Utc>>, DbError> {
    let ms: Option<i64> = conn
        .query_row(ANCHOR_SQL, [], |r| Ok(coerce_opt_i64(r.get_ref(0)?)))
        .map_err(|e| query_err(&e))?;
    Ok(ms.and_then(|ms| Utc.timestamp_millis_opt(ms).single()))
}

fn coerce_opt_i64(v: ValueRef<'_>) -> Option<i64> {
    (!matches!(v, ValueRef::Null)).then(|| coerce_i64(v))
}

/// Query 2: assistant messages created at or after `start_ms`. Rows with an
/// empty provider or model, or an unrepresentable time, are skipped; a
/// failing step is an error, never a partial read.
///
/// # Errors
/// [`DbError::QueryFailed`].
pub fn read_assistant_message_rows(
    conn: &Connection,
    start_ms: i64,
) -> Result<Vec<MessageRow>, DbError> {
    let mut stmt = conn.prepare(ROWS_SQL).map_err(|e| query_err(&e))?;
    let mut rows = stmt.query([start_ms]).map_err(|e| query_err(&e))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| query_err(&e))? {
        let get = |i| row.get_ref(i).map_err(|e| query_err(&e));
        let provider_id = coerce_text(get(2)?);
        let model_id = coerce_text(get(3)?);
        let Some(date) = Utc.timestamp_millis_opt(coerce_i64(get(0)?)).single() else {
            continue;
        };
        if provider_id.is_empty() || model_id.is_empty() {
            continue;
        }
        out.push(MessageRow {
            date,
            session_id: coerce_text(get(1)?),
            provider_id,
            model_id,
            cost_usd: coerce_f64(get(4)?),
            tokens: Tokens {
                input: coerce_i64(get(5)?),
                output: coerce_i64(get(6)?),
                cache_read: coerce_i64(get(7)?),
                cache_write: coerce_i64(get(8)?),
            },
        });
    }
    Ok(out)
}

/// Start of the scan window for `now`.
pub fn scan_start(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::days(SCAN_LOOKBACK_DAYS)
}

/// Modification time and size of one file; `None` when it is missing.
type FileStamp = Option<(Option<SystemTime>, u64)>;

/// Change detector for a WAL database: stamps of the db and its `-wal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    db: FileStamp,
    wal: FileStamp,
}

fn stamp(path: &Path) -> FileStamp {
    std::fs::metadata(path)
        .ok()
        .map(|m| (m.modified().ok(), m.len()))
}

impl Fingerprint {
    /// Fingerprint of `db` and `<db>-wal` as they are now.
    pub fn of(db: &Path) -> Self {
        let mut wal = db.as_os_str().to_owned();
        wal.push("-wal");
        Self {
            db: stamp(db),
            wal: stamp(Path::new(&wal)),
        }
    }
}

#[derive(Debug)]
struct CacheEntry {
    path: PathBuf,
    fingerprint: Fingerprint,
    since: DateTime<Utc>,
    scan: Arc<Scan>,
}

/// Scan cache keyed on database path and fingerprint.
#[derive(Debug, Default)]
pub struct ScanCache {
    entry: Mutex<Option<CacheEntry>>,
    loads: Mutex<u32>,
}

impl ScanCache {
    /// Number of times the database was actually read (for tests).
    pub fn loads(&self) -> u32 {
        *self.loads.lock().expect("loads lock")
    }
}

/// Loads a scan from `db`, reusing the cache when the path and fingerprint
/// match and the cached window starts no later than `since`.
///
/// # Errors
/// [`DbError`] when the file is missing, cannot be opened or a query fails.
pub fn load_scan(
    db: &Path,
    since: DateTime<Utc>,
    cache: &ScanCache,
) -> Result<Arc<Scan>, DbError> {
    if !db.is_file() {
        return Err(DbError::Missing(db.display().to_string()));
    }
    let fingerprint = Fingerprint::of(db);
    {
        let guard = cache.entry.lock().expect("cache lock");
        if let Some(e) = guard.as_ref()
            && e.path == db
            && e.fingerprint == fingerprint
            && e.since <= since
        {
            return Ok(Arc::clone(&e.scan));
        }
    }
    let conn = open_read_only(db)?;
    let scan = Arc::new(Scan {
        subscribed_at: earliest_go_session_date(&conn)?,
        rows: read_assistant_message_rows(&conn, since.timestamp_millis())?,
    });
    drop(conn);
    *cache.loads.lock().expect("loads lock") += 1;
    *cache.entry.lock().expect("cache lock") = Some(CacheEntry {
        path: db.to_path_buf(),
        fingerprint,
        since,
        scan: Arc::clone(&scan),
    });
    Ok(scan)
}
