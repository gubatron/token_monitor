//! Local history of weekly snapshots (`grok_history.dat`, spec 02 section 1.4).
//!
//! One end-of-day row per calendar day and account, persisted as
//! `{"version":1,"rows":[...]}`. The row format matches the macOS app (dates
//! as Apple reference-date seconds), so a file moved between platforms loads.
//!
//! The store is synchronous and single-owner. The 400 ms write debounce is the
//! caller's job: call [`GrokHistoryStore::flush`] when the debounce fires (and
//! on exit); [`GrokHistoryStore::is_dirty`] says whether a write is pending.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokenmon_core::Clock;

use crate::calendar::{DayCalendar, LocalCalendar, seconds_between};
use crate::models::WeeklyUsageSnapshot;

/// How long the owner should wait after an append before flushing.
pub const FLUSH_DEBOUNCE: Duration = Duration::from_millis(400);

/// Most rows kept in [`GrokHistoryStore::recent`] (the disk keeps all).
pub const RECENT_CAP: usize = 200;

/// File name of the history within the data directory.
pub const HISTORY_FILE_NAME: &str = "grok_history.dat";

const NOISE_PERCENT: f64 = 0.05;

/// What a backing store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackingRead {
    /// Nothing saved yet.
    Missing,
    /// The saved text.
    Text(String),
    /// A file exists but could not be read.
    Unreadable,
}

/// Where the history text is persisted.
pub trait HistoryBacking: Send + Sync {
    /// Reads the saved text.
    fn read(&self) -> BackingRead;
    /// Replaces the saved text; `false` on failure (the store retries later).
    fn write(&self, text: &str) -> bool;
}

/// File-backed [`HistoryBacking`]: atomic replace (temp file, then rename),
/// owner-only permissions on Unix.
///
/// A private stand-in for the shared core `FileBackedStringStore` until that
/// lands.
#[derive(Debug, Clone)]
pub struct FileBacking {
    path: PathBuf,
}

impl FileBacking {
    /// A backing at `dir/grok_history.dat`.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join(HISTORY_FILE_NAME),
        }
    }

    /// The history file path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl HistoryBacking for FileBacking {
    fn read(&self) -> BackingRead {
        match fs::read(&self.path) {
            Ok(bytes) => {
                String::from_utf8(bytes).map_or(BackingRead::Unreadable, BackingRead::Text)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BackingRead::Missing,
            Err(_) => BackingRead::Unreadable,
        }
    }

    fn write(&self, text: &str) -> bool {
        write_atomic(&self.path, text.as_bytes()).is_ok()
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("dat.tmp");
    let mut file = fs::File::create(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)
}

#[derive(Serialize, Deserialize)]
struct Payload {
    version: u32,
    rows: Vec<WeeklyUsageSnapshot>,
}

/// History of weekly snapshots with an active account.
pub struct GrokHistoryStore {
    backing: Option<Box<dyn HistoryBacking>>,
    clock: Arc<dyn Clock>,
    rows: Vec<WeeklyUsageSnapshot>,
    recent: Vec<WeeklyUsageSnapshot>,
    active_account: Option<String>,
    store_failed: bool,
    dirty: bool,
}

impl GrokHistoryStore {
    /// Opens the store. With `backing: None` the history lives in memory only.
    ///
    /// If the saved text cannot be decoded, [`Self::store_failed`] is set, the
    /// store runs in memory and never overwrites the bad file.
    pub fn open(backing: Option<Box<dyn HistoryBacking>>, clock: Arc<dyn Clock>) -> Self {
        let mut store = Self {
            backing,
            clock,
            rows: Vec::new(),
            recent: Vec::new(),
            active_account: None,
            store_failed: false,
            dirty: false,
        };
        store.load();
        store.reload();
        store
    }

    /// An in-memory store (tests, or when no data directory exists).
    pub fn in_memory(clock: Arc<dyn Clock>) -> Self {
        Self::open(None, clock)
    }

    fn load(&mut self) {
        let Some(backing) = &self.backing else { return };
        match backing.read() {
            BackingRead::Missing => {}
            BackingRead::Unreadable => self.store_failed = true,
            BackingRead::Text(text) => match serde_json::from_str::<Payload>(&text) {
                Ok(payload) => {
                    self.rows = payload.rows;
                    self.rows.sort_by_key(|r| r.fetched_at);
                }
                Err(e) => {
                    tracing::error!("Saved Grok history could not be read ({e}); using memory.");
                    self.store_failed = true;
                }
            },
        }
    }

    /// Rows of the active account, newest first, at most [`RECENT_CAP`].
    pub fn recent(&self) -> &[WeeklyUsageSnapshot] {
        &self.recent
    }

    /// The account whose rows [`Self::recent`] holds (`None` is the account
    /// with an unknown email).
    pub fn active_account(&self) -> Option<&str> {
        self.active_account.as_deref()
    }

    /// True when the saved history could not be read and is not being kept.
    pub fn store_failed(&self) -> bool {
        self.store_failed
    }

    /// True when rows changed since the last successful write.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Switches [`Self::recent`] to `account`'s rows.
    pub fn set_active_account(&mut self, account: Option<&str>) {
        if self.active_account.as_deref() != account {
            self.active_account = account.map(str::to_owned);
            self.reload();
        }
    }

    /// Appends `snapshot`, collapsing same-day polls of one account into one
    /// end-of-day row, and makes its account the active one.
    ///
    /// A same-day, same-usage change under 60 s is noise and ignored; an idle
    /// poll identical to the day's row leaves it (and the disk) untouched.
    pub fn append(&mut self, snapshot: WeeklyUsageSnapshot) {
        self.set_active_account(snapshot.account_email.clone().as_deref());
        let cal = LocalCalendar::new(self.clock.as_ref());
        if let Some(last) = self.recent.first() {
            let same_day = cal.same_day(last.fetched_at, snapshot.fetched_at);
            let tiny = (last.used_percent - snapshot.used_percent).abs() < NOISE_PERCENT
                && seconds_between(last.fetched_at, snapshot.fetched_at).abs() < 60.0;
            if same_day && (tiny || has_same_usage(last, &snapshot)) {
                return;
            }
        }
        self.rows.retain(|r| {
            !(r.account_email == snapshot.account_email
                && cal.same_day(r.fetched_at, snapshot.fetched_at))
        });
        let index = self
            .rows
            .iter()
            .position(|r| r.fetched_at > snapshot.fetched_at)
            .unwrap_or(self.rows.len());
        self.rows.insert(index, snapshot);
        self.dirty = true;
        self.reload();
    }

    /// Writes the rows when they changed. A history that could not be read is
    /// never written over. `dirty` stays set when the write fails so the next
    /// flush retries.
    pub fn flush(&mut self) {
        if !self.dirty || self.store_failed {
            return;
        }
        let Some(backing) = &self.backing else { return };
        let payload = Payload {
            version: 1,
            rows: self.rows.clone(),
        };
        let Ok(text) = serde_json::to_string(&payload) else {
            tracing::error!("Grok history encode failed");
            return;
        };
        if backing.write(&text) {
            self.dirty = false;
        }
    }

    /// Every saved row of every account, oldest first.
    pub fn all_snapshots(&self) -> &[WeeklyUsageSnapshot] {
        &self.rows
    }

    /// Deletes all rows and writes the empty history at once.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.recent.clear();
        self.dirty = true;
        self.flush();
    }

    fn reload(&mut self) {
        self.recent = self
            .rows
            .iter()
            .rev()
            .filter(|r| r.account_email == self.active_account)
            .take(RECENT_CAP)
            .cloned()
            .collect();
    }
}

/// `next` records nothing new over `current`.
fn has_same_usage(current: &WeeklyUsageSnapshot, next: &WeeklyUsageSnapshot) -> bool {
    (current.used_percent - next.used_percent).abs() < NOISE_PERCENT
        && (current.remaining_percent - next.remaining_percent).abs() < NOISE_PERCENT
        && current.resets_at == next.resets_at
        && current.products == next.products
        && current.extra_credits_balance == next.extra_credits_balance
        && current.account_email == next.account_email
}
