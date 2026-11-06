//! Incremental cache of parsed transcript files and per-calendar tallies.
//!
//! Transcripts are append-only, so a file that grew is read from where the
//! last read stopped; a file that shrank, was replaced, or whose last read did
//! not stop after a newline is parsed again in full. Running totals take only
//! the new entries and are rebuilt from every file's entries when entries were
//! dropped (a file removed or parsed again).
//!
//! One running tally is kept per calendar asked for (the ledger's UTC days and
//! the bars' local days), each brought up to date from a shared log of the
//! entries added since the slowest tally last caught up.
//!
//! Reads are byte-offset based and CRLF-safe: lines split on `\n` only, and
//! the append path re-reads the last 64 bytes before the saved offset (which
//! must end in `\n`) to notice a rewritten file.
//! A partly written trailing line (with or without a `\r`) is never consumed
//! until it is valid JSON or its newline arrives.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::line::{Entry, parse_complete_lines};
use tokenmon_core::usage::{Calendar, DayKey};

use crate::tokens::{DailyTokens, ModelTokens};

/// Calendars beyond this many drop every other tally so the shared log cannot
/// grow unbounded.
const MAX_TALLIES: usize = 4;
/// Directory depth cap for the transcript walk.
const MAX_WALK_DEPTH: usize = 64;
/// Bytes before the saved offset re-read to confirm the file was only
/// appended to (a superset of the Swift one-byte newline check).
const TAIL_CHECK_BYTES: usize = 64;

fn last_bytes(data: &[u8]) -> Vec<u8> {
    data[data.len().saturating_sub(TAIL_CHECK_BYTES)..].to_vec()
}

/// Identity of a calendar for the cache: tallies are kept per distinct
/// calendar. Every `Clock`-driven calendar is "the local calendar" (one
/// machine has one), every fixed offset is its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CalendarId {
    Fixed(i32),
    Local,
}

impl CalendarId {
    fn of(calendar: &Calendar<'_>) -> Self {
        match calendar {
            Calendar::Fixed(offset) => Self::Fixed(offset.local_minus_utc()),
            Calendar::Clock(_) => Self::Local,
        }
    }
}

/// Running per-day, per-model totals that count each response once.
#[derive(Debug, Clone)]
pub struct DailyTokenTally {
    days: DailyTokens,
    seen: HashSet<String>,
}

impl Default for DailyTokenTally {
    fn default() -> Self {
        Self::new()
    }
}

impl DailyTokenTally {
    /// An empty tally.
    pub fn new() -> Self {
        Self {
            days: BTreeMap::new(),
            seen: HashSet::new(),
        }
    }

    /// Adds `entry` unless its dedupe key was already seen (dedupe is global
    /// across files and days within one tally). Entries without a key always
    /// count.
    pub fn add(&mut self, entry: &Entry, calendar: &Calendar<'_>) {
        if let Some(key) = &entry.dedupe_key
            && !self.seen.insert(key.clone())
        {
            return;
        }
        let counts = ModelTokens {
            cache_read_tokens: entry.cache_read_tokens,
            cache_write_1h_tokens: entry.cache_write_1h_tokens,
            cache_write_5m_tokens: entry.cache_write_5m_tokens,
            input_tokens: entry.input_tokens,
            output_tokens: entry.output_tokens,
            response_count: 1,
        };
        let slot = self
            .days
            .entry(DayKey::for_instant(entry.date, calendar))
            .or_default()
            .entry(entry.model.clone())
            .or_default();
        *slot = slot.plus(&counts);
    }

    /// The totals so far.
    pub fn days(&self) -> &DailyTokens {
        &self.days
    }
}

/// Totals per day and model over entries, counting each response once.
pub fn daily_tokens_from_entries(entries: &[Entry], calendar: &Calendar<'_>) -> DailyTokens {
    let mut tally = DailyTokenTally::new();
    for e in entries {
        tally.add(e, calendar);
    }
    tally.days
}

#[derive(Debug, Clone)]
struct Item {
    root: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
    file_id: Option<u64>,
    parsed_bytes: usize,
    /// Last [`TAIL_CHECK_BYTES`] bytes of the parsed prefix; the append path
    /// verifies them on disk to notice a rewritten file.
    tail: Vec<u8>,
    entries: Vec<Entry>,
}

struct TallyState {
    tally: DailyTokenTally,
    /// Count of entries ever added (`added_count`) this tally includes.
    applied: usize,
}

#[derive(Default)]
struct Inner {
    items: HashMap<PathBuf, Item>,
    tallies: HashMap<CalendarId, TallyState>,
    /// Entries added since the slowest tally caught up; `added[0]` is entry
    /// number `added_base`.
    added: Vec<Entry>,
    added_base: usize,
    needs_rebuild: bool,
}

impl Inner {
    fn added_count(&self) -> usize {
        self.added_base + self.added.len()
    }

    fn rebuilt_tally(&self, calendar: &Calendar<'_>) -> DailyTokenTally {
        let mut paths: Vec<&PathBuf> = self.items.keys().collect();
        paths.sort();
        let mut tally = DailyTokenTally::new();
        for path in paths {
            for e in &self.items[path].entries {
                tally.add(e, calendar);
            }
        }
        tally
    }

    fn trim_added(&mut self) {
        let slowest = self
            .tallies
            .values()
            .map(|t| t.applied)
            .min()
            .unwrap_or_else(|| self.added_count());
        if slowest > self.added_base {
            self.added.drain(..slowest - self.added_base);
            self.added_base = slowest;
        }
    }
}

/// Parsed transcript files keyed by path, plus the running totals over them.
///
/// Thread-safe; the lock is never held across file I/O.
#[derive(Default)]
pub struct ClaudeCodeFileCache {
    inner: Mutex<Inner>,
}

impl ClaudeCodeFileCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Brings the cache up to date with every `.jsonl` file under `directory`
    /// and drops files there that no longer exist.
    ///
    /// Hidden files and directories (names starting with `.`) are skipped and
    /// symlinks are not followed.
    pub fn update(&self, directory: &Path) {
        let mut files = Vec::new();
        walk(directory, 0, &mut files);
        let seen: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        for f in files {
            self.refresh(directory, f);
        }
        let mut g = self.lock();
        let before = g.items.len();
        g.items
            .retain(|p, i| i.root != directory || seen.contains(p));
        if g.items.len() != before {
            g.needs_rebuild = true;
        }
    }

    /// Totals per day and model over every cached file, in `calendar`.
    pub fn daily_tokens(&self, calendar: &Calendar<'_>) -> DailyTokens {
        let id = CalendarId::of(calendar);
        let mut g = self.lock();
        if g.needs_rebuild {
            g.tallies.clear();
            g.needs_rebuild = false;
        }
        let count = g.added_count();
        if g.tallies.contains_key(&id) {
            let base = g.added_base;
            let Inner { tallies, added, .. } = &mut *g;
            if let Some(state) = tallies.get_mut(&id) {
                for e in &added[state.applied - base..] {
                    state.tally.add(e, calendar);
                }
                state.applied = count;
            }
        } else {
            if g.tallies.len() >= MAX_TALLIES {
                g.tallies.clear();
            }
            let tally = g.rebuilt_tally(calendar);
            g.tallies.insert(
                id,
                TallyState {
                    tally,
                    applied: count,
                },
            );
        }
        g.trim_added();
        g.tallies
            .get(&id)
            .map(|s| s.tally.days.clone())
            .unwrap_or_default()
    }

    fn refresh(&self, root: &Path, f: FoundFile) {
        let cached = self.lock().items.get(&f.path).cloned();
        if let Some(c) = &cached
            && c.modified == f.modified
            && c.size == f.size
            && c.file_id == f.file_id
        {
            return;
        }
        if let Some(c) = &cached
            && c.file_id == f.file_id
            && c.parsed_bytes > 0
            && c.tail.last() == Some(&b'\n')
            && f.size >= c.parsed_bytes as u64
            && let Some(appended) = read_appended(&f.path, c.parsed_bytes, &c.tail)
        {
            let (entries, consumed) = parse_complete_lines(&appended);
            let tail = last_bytes(&[c.tail.as_slice(), &appended[..consumed]].concat());
            let mut all = c.entries.clone();
            all.extend(entries.iter().cloned());
            let item = Item {
                root: root.to_path_buf(),
                modified: f.modified,
                size: f.size,
                file_id: f.file_id,
                parsed_bytes: c.parsed_bytes + consumed,
                tail,
                entries: all,
            };
            let mut g = self.lock();
            g.items.insert(f.path, item);
            g.added.extend(entries);
            return;
        }
        let data = fs::read(&f.path).unwrap_or_default();
        let (entries, consumed) = parse_complete_lines(&data);
        let item = Item {
            root: root.to_path_buf(),
            modified: f.modified,
            size: f.size,
            file_id: f.file_id,
            parsed_bytes: consumed,
            tail: last_bytes(&data[..consumed]),
            entries: entries.clone(),
        };
        let mut g = self.lock();
        g.items.insert(f.path, item);
        if cached.is_none() {
            g.added.extend(entries);
        } else {
            g.needs_rebuild = true;
        }
    }
}

/// The bytes after `offset`, or `None` when the saved `tail` (the bytes just
/// before `offset`) no longer matches the file, i.e. it was rewritten.
fn read_appended(path: &Path, offset: usize, tail: &[u8]) -> Option<Vec<u8>> {
    let start = offset.checked_sub(tail.len())?;
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start as u64)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    buf.starts_with(tail).then(|| buf.split_off(tail.len()))
}

struct FoundFile {
    path: PathBuf,
    modified: Option<SystemTime>,
    size: u64,
    file_id: Option<u64>,
}

/// File identity (inode on Unix). `None` on Windows: std's
/// `MetadataExt::file_index` is unstable, so replacement detection there
/// relies on the tail check in [`read_appended`] plus size/mtime.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn file_id(md: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(md.ino())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn file_id(_md: &fs::Metadata) -> Option<u64> {
    None
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<FoundFile>) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, depth + 1, out);
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "jsonl")
            && let Ok(md) = entry.metadata()
        {
            out.push(FoundFile {
                path,
                modified: md.modified().ok(),
                size: md.len(),
                file_id: file_id(&md),
            });
        }
    }
}
