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
//!
//! # Memory bounds
//!
//! Transcripts are user-writable and can reach gigabytes, so nothing here
//! holds a whole file or every parsed entry:
//!
//! * Files are **streamed** in [`BATCH_BYTES`] batches of whole lines; a single
//!   line over [`MAX_LINE_BYTES`] is skipped, never buffered.
//! * Per file only metadata and a 64-byte tail are retained; entries are
//!   folded into the running tallies and dropped. A rebuild (new calendar, a
//!   removed or replaced file) re-streams the files from disk, up to the bytes
//!   already parsed.
//! * The log of entries awaiting the slowest tally holds at most
//!   [`MAX_ADDED_LOG`] entries; beyond that the tallies are rebuilt from disk.
//! * Dedupe keys are kept as 64-bit hashes, at most [`MAX_SEEN_KEYS`] per
//!   tally; past that, further keys are counted without being remembered.
//! * Files over [`MAX_FILE_BYTES`] are ignored and the walk stops at
//!   [`MAX_FILES`] files.
//!
//! Worst case retained memory is therefore about 100 MiB (the entry log plus
//! the dedupe hashes of all [`MAX_TALLIES`] tallies); a typical machine uses a
//! few MiB. Peak working memory while streaming is [`BATCH_BYTES`] plus one
//! line.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
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
/// Whole lines are parsed in batches of about this many bytes.
pub const BATCH_BYTES: usize = 1 << 20;
/// A single transcript line over this is skipped instead of buffered.
pub const MAX_LINE_BYTES: usize = 16 << 20;
/// Files over this size are ignored (a runaway or planted file).
pub const MAX_FILE_BYTES: u64 = 2 << 30;
/// The transcript walk stops collecting after this many files.
pub const MAX_FILES: usize = 50_000;
/// Cap on entries waiting for the slowest tally; past it, tallies are rebuilt
/// from disk instead of fed from the log.
pub const MAX_ADDED_LOG: usize = 100_000;
/// Cap on remembered dedupe keys per tally.
pub const MAX_SEEN_KEYS: usize = 4_000_000;
/// Chunk size when skipping an oversized line.
const DISCARD_CHUNK: u64 = 64 * 1024;

/// Appends the last [`TAIL_CHECK_BYTES`] of `chunk` to the rolling `tail`.
fn push_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(&chunk[chunk.len().saturating_sub(TAIL_CHECK_BYTES)..]);
    if tail.len() > TAIL_CHECK_BYTES {
        tail.drain(..tail.len() - TAIL_CHECK_BYTES);
    }
}

/// Parses the whole lines of `batch` into `on_entry` and empties it.
fn flush(
    batch: &mut Vec<u8>,
    consumed: &mut usize,
    tail: &mut Vec<u8>,
    on_entry: &mut dyn FnMut(Entry),
) {
    let (entries, used) = parse_complete_lines(batch);
    for e in entries {
        on_entry(e);
    }
    *consumed += used;
    push_tail(tail, &batch[..used]);
    batch.clear();
}

/// Streams `reader` line by line, feeding every usage entry to `on_entry`
/// without ever holding more than a batch. Returns the bytes consumed and
/// keeps `tail` as the last 64 consumed bytes (seeded by the caller with the
/// previous tail on the append path).
///
/// Same consumption rule as [`parse_complete_lines`]: every newline-terminated
/// line counts; an unterminated final line counts only when it is whole JSON.
/// A line over [`MAX_LINE_BYTES`] is skipped (consumed) once its newline is
/// seen and left unconsumed while it is still being written.
fn stream_entries(
    mut reader: impl BufRead,
    tail: &mut Vec<u8>,
    on_entry: &mut dyn FnMut(Entry),
) -> usize {
    let mut consumed = 0usize;
    let mut batch: Vec<u8> = Vec::new();
    let mut line: Vec<u8> = Vec::new();

    loop {
        line.clear();
        let Ok(n) = (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        else {
            break;
        };
        if n == 0 {
            break;
        }
        let terminated = line.last() == Some(&b'\n');
        if !terminated && line.len() > MAX_LINE_BYTES {
            // Oversized line: never buffer it; skip to its newline.
            flush(&mut batch, &mut consumed, tail, on_entry);
            let saved_tail = tail.clone();
            let mut skipped = n;
            push_tail(tail, &line);
            let mut ended = false;
            loop {
                line.clear();
                let m = (&mut reader)
                    .take(DISCARD_CHUNK)
                    .read_until(b'\n', &mut line)
                    .unwrap_or(0);
                if m == 0 {
                    break;
                }
                skipped += m;
                push_tail(tail, &line);
                if line.last() == Some(&b'\n') {
                    ended = true;
                    break;
                }
            }
            if !ended {
                // Still being written: leave it for a later read.
                *tail = saved_tail;
                return consumed;
            }
            consumed += skipped;
            continue;
        }
        if !terminated {
            // The unterminated last line: consumed only when whole JSON.
            flush(&mut batch, &mut consumed, tail, on_entry);
            std::mem::swap(&mut batch, &mut line);
            flush(&mut batch, &mut consumed, tail, on_entry);
            return consumed;
        }
        batch.extend_from_slice(&line);
        if batch.len() >= BATCH_BYTES {
            flush(&mut batch, &mut consumed, tail, on_entry);
        }
    }
    flush(&mut batch, &mut consumed, tail, on_entry);
    consumed
}

/// Hash of a dedupe key. `DefaultHasher::new()` uses fixed keys, so equal keys
/// hash equally for the life of the process; a 64-bit collision (about
/// `n^2 / 2^65` over `n` keys) would drop one response from a total.
fn key_hash(key: &str) -> u64 {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
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
    seen: HashSet<u64>,
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
        if let Some(key) = &entry.dedupe_key {
            let hash = key_hash(key);
            if self.seen.contains(&hash) {
                return;
            }
            // Past the cap a key is counted but not remembered (documented bound).
            if self.seen.len() < MAX_SEEN_KEYS {
                self.seen.insert(hash);
            }
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

    /// Re-streams every cached file (up to the bytes already parsed) into a
    /// fresh tally. File I/O under the lock keeps the result consistent with
    /// `items` and the entry log.
    fn rebuilt_tally(&self, calendar: &Calendar<'_>) -> DailyTokenTally {
        let mut paths: Vec<&PathBuf> = self.items.keys().collect();
        paths.sort();
        let mut tally = DailyTokenTally::new();
        for path in paths {
            let item = &self.items[path];
            let Ok(file) = File::open(path) else { continue };
            let reader = BufReader::new(file.take(item.parsed_bytes as u64));
            stream_entries(reader, &mut Vec::new(), &mut |e| tally.add(&e, calendar));
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
/// Thread-safe. Memory is bounded (see the module docs); reads of a file
/// happen outside the lock, only a rebuild of the tallies streams under it.
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
        let (cached, collecting) = {
            let g = self.lock();
            (g.items.get(&f.path).cloned(), !g.tallies.is_empty())
        };
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
            && let Some(reader) = open_appended(&f.path, c.parsed_bytes, &c.tail)
        {
            let mut tail = c.tail.clone();
            let mut sink = EntryLog::new(collecting);
            let consumed = stream_entries(reader, &mut tail, &mut |e| sink.push(e));
            let item = Item {
                root: root.to_path_buf(),
                modified: f.modified,
                size: f.size,
                file_id: f.file_id,
                parsed_bytes: c.parsed_bytes + consumed,
                tail,
            };
            self.commit(f.path, item, sink, false);
            return;
        }
        // A first parse feeds the tallies through the log; a re-parse of a
        // changed file rebuilds them instead (entries may have been dropped).
        let replaced = cached.is_some();
        let mut sink = EntryLog::new(collecting && !replaced);
        let mut tail = Vec::new();
        let consumed = match File::open(&f.path) {
            Ok(file) => stream_entries(BufReader::new(file), &mut tail, &mut |e| sink.push(e)),
            Err(_) => 0,
        };
        let item = Item {
            root: root.to_path_buf(),
            modified: f.modified,
            size: f.size,
            file_id: f.file_id,
            parsed_bytes: consumed,
            tail,
        };
        self.commit(f.path, item, sink, replaced);
    }

    /// Stores a refreshed item and routes its entries to the tallies.
    fn commit(&self, path: PathBuf, item: Item, sink: EntryLog, replaced: bool) {
        let mut g = self.lock();
        g.items.insert(path, item);
        if replaced || sink.overflow {
            g.needs_rebuild = true;
        } else if !sink.active {
            // Tallies appeared while this file streamed: they never saw it.
            if !g.tallies.is_empty() {
                g.needs_rebuild = true;
            }
        } else if g.added.len() + sink.entries.len() > MAX_ADDED_LOG {
            g.needs_rebuild = true;
        } else {
            g.added.extend(sink.entries);
        }
    }
}

/// Entries of one refreshed file waiting for the tallies, bounded by
/// [`MAX_ADDED_LOG`].
struct EntryLog {
    /// Collecting at all (some tally exists to feed).
    active: bool,
    entries: Vec<Entry>,
    /// More than the cap arrived: the tallies are rebuilt from disk instead.
    overflow: bool,
}

impl EntryLog {
    fn new(active: bool) -> Self {
        Self {
            active,
            entries: Vec::new(),
            overflow: false,
        }
    }

    fn push(&mut self, entry: Entry) {
        if !self.active || self.overflow {
            return;
        }
        if self.entries.len() >= MAX_ADDED_LOG {
            self.overflow = true;
            self.entries = Vec::new();
        } else {
            self.entries.push(entry);
        }
    }
}

/// A reader positioned just after `offset`, or `None` when the saved `tail`
/// (the bytes just before `offset`) no longer matches the file, i.e. it was
/// rewritten.
fn open_appended(path: &Path, offset: usize, tail: &[u8]) -> Option<BufReader<File>> {
    let start = offset.checked_sub(tail.len())?;
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start as u64)).ok()?;
    let mut check = vec![0u8; tail.len()];
    file.read_exact(&mut check).ok()?;
    (check == tail).then(|| BufReader::new(file))
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
        if out.len() >= MAX_FILES {
            return;
        }
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
            && md.len() <= MAX_FILE_BYTES
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
