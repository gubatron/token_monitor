//! Forever-ledger of Claude Code daily token totals (format version 2).
//!
//! Claude Code deletes transcripts after about 30 days; the ledger keeps the
//! per-day totals. Days are UTC (local days would double count after a time
//! zone change). The file is `claude_code_daily_tokens.dat` in the app data
//! directory, compact JSON with sorted keys:
//! `{"days":{"2026-10-01":{"claude-opus-5-5":{...}}},"version":2}`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokenmon_core::storage::{NoopPermissions, write_atomic};

use crate::tokens::DailyTokens;

/// Ledger file name inside the app data directory.
pub const LEDGER_FILE_NAME: &str = "claude_code_daily_tokens.dat";
/// Largest ledger file read. A real ledger (days x models) is a few hundred
/// KiB; anything bigger is a runaway or planted file and is treated as
/// unreadable instead of being loaded into memory.
pub const MAX_LEDGER_BYTES: u64 = 32 << 20;
/// The only format version this code reads or writes.
pub const LEDGER_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct Payload {
    days: DailyTokens,
    version: u32,
}

/// Decodes a ledger payload. Anything unreadable, a missing count field, or a
/// version other than 2 yields an empty ledger.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::ledger::decode_ledger;
/// assert!(decode_ledger(b"not json").is_empty());
/// assert!(decode_ledger(br#"{"days":{},"version":1}"#).is_empty());
/// ```
pub fn decode_ledger(bytes: &[u8]) -> DailyTokens {
    match serde_json::from_slice::<Payload>(bytes) {
        Ok(p) if p.version == LEDGER_VERSION => p.days,
        Ok(_) => DailyTokens::new(),
        Err(e) => {
            tracing::error!("claude ledger unreadable, starting empty: {e}");
            DailyTokens::new()
        }
    }
}

/// Encodes `days` as a version-2 payload (sorted keys, compact).
pub fn encode_ledger(days: &DailyTokens) -> Vec<u8> {
    serde_json::to_vec(&Payload {
        days: days.clone(),
        version: LEDGER_VERSION,
    })
    .unwrap_or_default()
}

/// Merges `scanned` into `saved`: union of days and models, fieldwise max
/// where both have a day/model.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::ledger::merge_days;
/// use tokenmon_provider_claude::DailyTokens;
/// assert!(merge_days(&DailyTokens::new(), &DailyTokens::new()).is_empty());
/// ```
pub fn merge_days(saved: &DailyTokens, scanned: &DailyTokens) -> DailyTokens {
    let mut merged = saved.clone();
    for (day, models) in scanned {
        let slot = merged.entry(day.clone()).or_default();
        for (model, tokens) in models {
            slot.entry(model.clone())
                .and_modify(|old| *old = old.fieldwise_max(tokens))
                .or_insert(*tokens);
        }
    }
    merged
}

/// The persistent ledger. Thread-safe; loads lazily on first merge.
pub struct ClaudeCodeUsageLedger {
    path: PathBuf,
    cached: Mutex<Option<DailyTokens>>,
}

impl ClaudeCodeUsageLedger {
    /// A ledger stored as [`LEDGER_FILE_NAME`] inside `dir` (the app data
    /// directory; injected so tests use a temp dir).
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(LEDGER_FILE_NAME),
            cached: Mutex::new(None),
        }
    }

    /// Merges a fresh scan into the saved days, persists when something
    /// changed, and returns the merged days.
    pub fn merge(&self, scanned: &DailyTokens) -> DailyTokens {
        let mut guard = self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = guard.take().unwrap_or_else(|| self.load());
        let merged = merge_days(&saved, scanned);
        if merged != saved {
            self.save(&merged);
        }
        *guard = Some(merged.clone());
        merged
    }

    fn load(&self) -> DailyTokens {
        read_bounded(&self.path, MAX_LEDGER_BYTES)
            .map(|b| decode_ledger(&b))
            .unwrap_or_default()
    }

    /// Atomic write (temp file then rename). Failures are logged, not fatal:
    /// the in-memory ledger stays correct and the next merge retries.
    fn save(&self, days: &DailyTokens) {
        if let Err(e) = write_atomic(&self.path, &encode_ledger(days), &NoopPermissions) {
            tracing::error!("claude ledger write failed: {e}");
        }
    }
}

/// Reads at most `max` bytes of `path`; a longer file is an error rather than
/// a truncated (and therefore misleading) read.
///
/// # Errors
///
/// The open or read error, or `InvalidData` when the file exceeds `max`.
pub fn read_bounded(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    File::open(path)?.take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file exceeds the size bound",
        ));
    }
    Ok(buf)
}
