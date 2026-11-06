//! Claude Code transcript (JSONL) line parser.
//!
//! Transcripts reach gigabytes and are user-writable, so every function here
//! is total: arbitrary bytes never panic. Lines are split on `\n` only (a
//! trailing `\r` stays in the line and is JSON whitespace), and only lines
//! containing `"usage"` are JSON-decoded.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::IgnoredAny;

/// One assistant response's usage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Entry {
    /// When the response was written.
    pub date: DateTime<Utc>,
    /// Model id (`unknown` when absent).
    pub model: String,
    /// Input tokens.
    pub input_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Cache read tokens.
    pub cache_read_tokens: i64,
    /// 5-minute cache write tokens.
    pub cache_write_5m_tokens: i64,
    /// 1-hour cache write tokens.
    pub cache_write_1h_tokens: i64,
    /// `message.id:requestId`. Claude Code writes one line per content block
    /// of a streamed response and resumed sessions copy earlier lines, so
    /// entries sharing a key are counted once.
    pub dedupe_key: Option<String>,
}

#[derive(Deserialize)]
struct TranscriptLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<TranscriptMessage>,
}

#[derive(Deserialize)]
struct TranscriptMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<TranscriptUsage>,
}

#[derive(Deserialize)]
struct TranscriptUsage {
    input_tokens: Option<f64>,
    output_tokens: Option<f64>,
    cache_read_input_tokens: Option<f64>,
    cache_creation_input_tokens: Option<f64>,
    cache_creation: Option<TranscriptCacheCreation>,
}

#[derive(Deserialize)]
struct TranscriptCacheCreation {
    ephemeral_1h_input_tokens: Option<f64>,
}

const USAGE_MARKER: &[u8] = b"\"usage\"";

/// A non-negative token count, 0 for missing/NaN/infinite/non-positive.
/// Saturates at `i64::MAX` (Swift would trap there).
fn count(v: Option<f64>) -> i64 {
    match v {
        Some(x) if x.is_finite() && x > 0.0 => x as i64,
        _ => 0,
    }
}

fn contains_marker(line: &[u8]) -> bool {
    line.windows(USAGE_MARKER.len()).any(|w| w == USAGE_MARKER)
}

/// Byte ranges of the lines (split on `\n`) that contain `"usage"`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::usage_line_ranges;
/// let data = b"a\n{\"usage\":1}\nb";
/// assert_eq!(usage_line_ranges(data), vec![2..13]);
/// ```
pub fn usage_line_ranges(data: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let end = data[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(data.len(), |p| start + p);
        if contains_marker(&data[start..end]) {
            ranges.push(start..end);
        }
        start = end + 1;
    }
    ranges
}

/// One transcript line to its usage entry; `None` for any other line.
///
/// Decoding is type-strict per declared field: a string where a number is
/// expected invalidates the whole line. `<synthetic>` replies are skipped.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::parse_line;
/// let line = br#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-1","usage":{"input_tokens":10,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_1h_input_tokens":70}}}}"#;
/// let e = parse_line(line).unwrap();
/// assert_eq!((e.cache_write_5m_tokens, e.cache_write_1h_tokens), (30, 70));
/// assert_eq!(e.dedupe_key.as_deref(), Some("m:r"));
/// assert!(parse_line(b"not json").is_none());
/// ```
pub fn parse_line(line: &[u8]) -> Option<Entry> {
    let root: TranscriptLine = serde_json::from_slice(line).ok()?;
    if root.kind.as_deref() != Some("assistant") {
        return None;
    }
    let message = root.message?;
    let usage = message.usage?;
    let date = DateTime::parse_from_rfc3339(root.timestamp.as_deref()?)
        .ok()?
        .with_timezone(&Utc);
    let model = message.model.unwrap_or_else(|| "unknown".to_owned());
    if model == "<synthetic>" {
        return None;
    }
    let dedupe_key = match (message.id, root.request_id) {
        (Some(m), Some(r)) => Some(format!("{m}:{r}")),
        _ => None,
    };
    let cache_write = count(usage.cache_creation_input_tokens);
    let one_hour = count(
        usage
            .cache_creation
            .and_then(|c| c.ephemeral_1h_input_tokens),
    )
    .min(cache_write);
    Some(Entry {
        date,
        model,
        input_tokens: count(usage.input_tokens),
        output_tokens: count(usage.output_tokens),
        cache_read_tokens: count(usage.cache_read_input_tokens),
        cache_write_5m_tokens: cache_write - one_hour,
        cache_write_1h_tokens: one_hour,
        dedupe_key,
    })
}

/// Every usage entry in a JSONL buffer, in file order.
pub fn parse_jsonl(data: &[u8]) -> Vec<Entry> {
    usage_line_ranges(data)
        .into_iter()
        .filter_map(|r| parse_line(&data[r]))
        .collect()
}

/// Parses the complete lines of `data`: every newline-terminated line plus a
/// final unterminated line when it is already whole JSON.
///
/// Returns the entries and the number of bytes consumed. A partly written
/// last line is left unconsumed for a later read. `consumed` is always
/// `<= data.len()` and ends right after a `\n` or at the end of `data`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::parse_complete_lines;
/// let (entries, consumed) = parse_complete_lines(b"{\"a\":1}\n{\"usage\":");
/// assert!(entries.is_empty());
/// assert_eq!(consumed, 8);
/// ```
pub fn parse_complete_lines(data: &[u8]) -> (Vec<Entry>, usize) {
    let tail_start = data.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let tail = &data[tail_start..];
    let end = if !tail.is_empty() && serde_json::from_slice::<IgnoredAny>(tail).is_ok() {
        data.len()
    } else {
        tail_start
    };
    (parse_jsonl(&data[..end]), end)
}
