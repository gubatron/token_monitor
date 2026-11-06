//! Memory bounds of the transcript cache (issue #48): files are streamed,
//! oversized lines are skipped, retained state is capped, and results stay
//! exactly what the old whole-file reader produced.
#![allow(clippy::unwrap_used)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use chrono::FixedOffset;
use tokenmon_core::usage::Calendar;
use tokenmon_provider_claude::cache::{BATCH_BYTES, MAX_ADDED_LOG, MAX_LINE_BYTES};
use tokenmon_provider_claude::ledger::{ClaudeCodeUsageLedger, MAX_LEDGER_BYTES, read_bounded};
use tokenmon_provider_claude::{ClaudeCodeFileCache, DailyTokens};

fn line(n: u32) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","requestId":"r{n}","message":{{"id":"m{n}","model":"claude-opus-4-1","usage":{{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":300,"cache_creation_input_tokens":40}}}}}}"#
    )
}

fn count(days: &DailyTokens) -> i64 {
    days.values()
        .flat_map(|m| m.values())
        .map(|t| t.response_count)
        .sum()
}

fn total(cache: &ClaudeCodeFileCache, dir: &Path, cal: &Calendar<'_>) -> i64 {
    cache.update(dir);
    count(&cache.daily_tokens(cal))
}

fn append(path: &Path, content: &str) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(content.as_bytes())
        .unwrap();
}

/// A file several batches long is counted exactly (batch boundaries never
/// split or lose a line), and so is a later append.
#[test]
fn a_multi_batch_file_is_streamed_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("big.jsonl");
    let one = line(0).len() + 1;
    let n = (3 * BATCH_BYTES / one) as u32 + 17;
    let mut body = String::new();
    for i in 0..n {
        body.push_str(&line(i));
        body.push('\n');
    }
    assert!(body.len() > 3 * BATCH_BYTES);
    fs::write(&f, &body).unwrap();
    let cache = ClaudeCodeFileCache::new();
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), i64::from(n));
    append(&f, &format!("{}\n", line(n)));
    assert_eq!(
        total(&cache, dir.path(), &Calendar::utc()),
        i64::from(n) + 1
    );
    // A second calendar rebuilds from disk, to the same totals.
    let tokyo = Calendar::fixed(FixedOffset::east_opt(9 * 3600).unwrap());
    assert_eq!(total(&cache, dir.path(), &tokyo), i64::from(n) + 1);
    // And duplicates are still counted once across a rebuild.
    fs::write(dir.path().join("dup.jsonl"), format!("{}\n", line(1))).unwrap();
    assert_eq!(
        total(&cache, dir.path(), &Calendar::utc()),
        i64::from(n) + 1
    );
}

/// A single line over the cap is skipped, never buffered, and its neighbours
/// still count. While it is still being written it is left unconsumed.
#[test]
fn an_oversized_line_is_skipped_not_buffered() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("huge.jsonl");
    let mut body = format!("{}\n", line(1));
    body.push_str(r#"{"type":"assistant","usage":"#);
    body.push_str(&"x".repeat(MAX_LINE_BYTES + 1024));
    body.push_str("\"}\n");
    body.push_str(&line(2));
    body.push('\n');
    fs::write(&f, &body).unwrap();
    let cache = ClaudeCodeFileCache::new();
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), 2);

    // Unterminated oversized tail: not consumed; completing it later works.
    let g = dir.path().join("growing.jsonl");
    let mut partial = format!("{}\n", line(10));
    partial.push_str(&"y".repeat(MAX_LINE_BYTES + 10));
    fs::write(&g, &partial).unwrap();
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), 3);
    append(&g, &format!("\n{}\n", line(11)));
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), 4);
}

/// More new entries than the entry log holds fall back to a rebuild from disk
/// instead of growing the log, with exact totals.
#[test]
fn an_overflowing_entry_log_rebuilds_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.jsonl");
    fs::write(&f, format!("{}\n", line(0))).unwrap();
    let cache = ClaudeCodeFileCache::new();
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), 1);
    let extra = MAX_ADDED_LOG as u32 + 50;
    let mut body = String::new();
    for i in 1..=extra {
        body.push_str(&line(i));
        body.push('\n');
    }
    append(&f, &body);
    assert_eq!(
        total(&cache, dir.path(), &Calendar::utc()),
        i64::from(extra) + 1
    );
    // Still exact on the next incremental poll.
    append(&f, &format!("{}\n", line(extra + 1)));
    assert_eq!(
        total(&cache, dir.path(), &Calendar::utc()),
        i64::from(extra) + 2
    );
}

/// A removed file leaves the totals (rebuild streams only what remains).
#[test]
fn a_removed_file_drops_out_of_the_totals() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.jsonl"), format!("{}\n", line(1))).unwrap();
    fs::write(dir.path().join("b.jsonl"), format!("{}\n", line(2))).unwrap();
    let cache = ClaudeCodeFileCache::new();
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), 2);
    fs::remove_file(dir.path().join("b.jsonl")).unwrap();
    assert_eq!(total(&cache, dir.path(), &Calendar::utc()), 1);
}

/// The ledger is read with a bound; an oversized file is unreadable, not
/// loaded.
#[test]
fn the_ledger_read_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.dat");
    fs::write(&p, [b'a'; 11]).unwrap();
    assert_eq!(read_bounded(&p, 11).unwrap().len(), 11);
    assert!(read_bounded(&p, 10).is_err());
    assert!(read_bounded(&dir.path().join("missing"), 10).is_err());

    // An oversized ledger file is treated like a corrupt one: empty, no OOM.
    let big = dir.path().join("claude_code_daily_tokens.dat");
    let f = fs::File::create(&big).unwrap();
    f.set_len(MAX_LEDGER_BYTES + 1).unwrap();
    drop(f);
    let ledger = ClaudeCodeUsageLedger::new(dir.path());
    assert!(ledger.merge(&DailyTokens::new()).is_empty());
}
