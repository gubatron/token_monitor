//! Ports of `ClaudeCodeLocalStatsTests.swift` (spec 03 section 7.3) plus
//! CRLF, partial-line and discovery-environment tests.
#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::FixedOffset;
use tokenmon_core::usage::{Calendar, DayKey};

use tokenmon_provider_claude::{
    ClaudeCodeFileCache, ClaudeEnvironment, parse_complete_lines, parse_jsonl, parse_line,
    projects_directories, scan_daily_tokens, usage_line_ranges,
};

/// A transcript line: opus-4-1, in10/out20/cr300/cw40, optional 1h split.
fn line(ts: &str, msg: &str, req: &str, model: &str, one_hour: Option<u64>) -> String {
    let cc = one_hour.map_or(String::new(), |h| {
        format!(r#","cache_creation":{{"ephemeral_1h_input_tokens":{h}}}"#)
    });
    format!(
        r#"{{"type":"assistant","timestamp":"{ts}","requestId":"{req}","message":{{"id":"{msg}","model":"{model}","usage":{{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":300,"cache_creation_input_tokens":40{cc}}}}}}}"#
    )
}

fn l(ts: &str, n: u32) -> String {
    line(
        ts,
        &format!("msg_{n}"),
        &format!("req_{n}"),
        "claude-opus-4-1",
        None,
    )
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn append(path: &Path, content: &str) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(content.as_bytes())
        .unwrap();
}

fn responses(cache: &ClaudeCodeFileCache, dir: &Path, cal: &Calendar<'_>) -> i64 {
    cache.update(dir);
    cache
        .daily_tokens(cal)
        .values()
        .flat_map(|m| m.values())
        .map(|t| t.response_count)
        .sum()
}

/// Only valid assistant usage lines parse; user lines, synthetic and junk are skipped.
#[test]
fn test_parses_assistant_usage_and_skips_other_lines() {
    let data = [
        r#"{"type":"user","message":{"usage":{}}}"#.to_owned(),
        l("2026-10-10T10:00:00Z", 1),
        line("2026-10-10T10:00:01Z", "s", "s", "<synthetic>", None),
        "not json \"usage\"".to_owned(),
    ]
    .join("\n");
    let entries = parse_jsonl(data.as_bytes());
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(
        (e.input_tokens, e.output_tokens, e.cache_read_tokens),
        (10, 20, 300)
    );
    assert_eq!((e.cache_write_5m_tokens, e.cache_write_1h_tokens), (40, 0));
    assert_eq!(e.dedupe_key.as_deref(), Some("msg_1:req_1"));
}

/// A sub-slice with a non-zero start works the same as a whole buffer.
#[test]
fn test_parses_a_slice_of_a_larger_buffer() {
    let data = format!(
        "garbage\n{}\n{}\n",
        l("2026-10-10T10:00:00Z", 1),
        l("2026-10-10T10:00:01Z", 2)
    );
    let start = data.find('\n').unwrap() + 1;
    let keys: Vec<_> = parse_jsonl(&data.as_bytes()[start..])
        .into_iter()
        .map(|e| e.dedupe_key.unwrap())
        .collect();
    assert_eq!(keys, ["msg_1:req_1", "msg_2:req_2"]);
}

/// Timestamps with and without fractional seconds both parse.
#[test]
fn test_parses_timestamps_with_and_without_fractional_seconds() {
    let a = parse_line(l("2026-10-10T10:00:01.500Z", 1).as_bytes()).unwrap();
    let b = parse_line(l("2026-10-10T10:00:01Z", 1).as_bytes()).unwrap();
    assert_eq!((a.date - b.date).num_milliseconds(), 500);
}

/// Cache writes split by lifetime.
#[test]
fn test_splits_cache_writes_by_lifetime() {
    let json = r#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"usage":{"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_1h_input_tokens":70}}}}"#;
    let e = parse_line(json.as_bytes()).unwrap();
    assert_eq!((e.cache_write_5m_tokens, e.cache_write_1h_tokens), (30, 70));
    assert_eq!(e.model, "unknown");
    assert!(e.dedupe_key.is_none());
}

/// 1-hour writes never exceed the total.
#[test]
fn test_one_hour_cache_writes_never_exceed_the_total() {
    let json = r#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"usage":{"cache_creation_input_tokens":10,"cache_creation":{"ephemeral_1h_input_tokens":50}}}}"#;
    let e = parse_line(json.as_bytes()).unwrap();
    assert_eq!((e.cache_write_5m_tokens, e.cache_write_1h_tokens), (0, 10));
}

/// A type mismatch on a declared field drops the whole line (Swift decoder parity).
#[test]
fn test_type_mismatch_on_declared_field_drops_the_line() {
    for bad in [
        r#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"usage":{"input_tokens":"5"}}}"#,
        r#"{"type":7,"timestamp":"2026-10-10T10:00:00Z","message":{"usage":{}}}"#,
        r#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"id":5,"usage":{}}}"#,
        r#"{"type":"assistant","timestamp":"yesterday","message":{"usage":{}}}"#,
    ] {
        assert!(parse_line(bad.as_bytes()).is_none(), "{bad}");
    }
}

/// Negative, huge and null counts never go negative or panic.
#[test]
fn test_counts_clamp_and_saturate() {
    let json = r#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"usage":{"input_tokens":-5,"output_tokens":1e30,"cache_read_input_tokens":null,"cache_creation_input_tokens":2.9}}}"#;
    let e = parse_line(json.as_bytes()).unwrap();
    assert_eq!(
        (e.input_tokens, e.output_tokens, e.cache_read_tokens),
        (0, i64::MAX, 0)
    );
    assert_eq!(e.cache_write_5m_tokens, 2);
}

/// Duplicate response keys count once and group by day and model.
#[test]
fn test_daily_tokens_dedupe_and_group_by_day_and_model() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("p/s.jsonl");
    let sonnet = line(
        "2026-10-10T12:00:00Z",
        "msg_9",
        "req_9",
        "claude-sonnet-4-5",
        None,
    );
    write(
        &p,
        &format!(
            "{}\n{}\n{}\n{}\n{}\n",
            l("2026-10-10T10:00:00Z", 1),
            l("2026-10-10T10:00:00Z", 1),
            l("2026-10-10T11:00:00Z", 2),
            sonnet,
            l("2026-10-11T01:00:00Z", 3)
        ),
    );
    let cache = ClaudeCodeFileCache::new();
    cache.update(dir.path());
    let days = cache.daily_tokens(&Calendar::utc());
    let opus = days[&DayKey::from_raw("2026-10-10")]["claude-opus-4-1"];
    assert_eq!(
        (
            opus.response_count,
            opus.input_tokens,
            opus.output_tokens,
            opus.total_tokens()
        ),
        (2, 20, 40, 740)
    );
    assert_eq!(
        days[&DayKey::from_raw("2026-10-10")]["claude-sonnet-4-5"].response_count,
        1
    );
    assert_eq!(
        days[&DayKey::from_raw("2026-10-11")]["claude-opus-4-1"].total_tokens(),
        370
    );
}

/// Day keys use the calendar's own day.
#[test]
fn test_daily_tokens_use_the_calendars_local_day() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("a.jsonl"),
        &format!("{}\n", l("2026-10-10T20:00:00Z", 1)),
    );
    let cache = ClaudeCodeFileCache::new();
    cache.update(dir.path());
    let tokyo = cache.daily_tokens(&Calendar::fixed(FixedOffset::east_opt(9 * 3600).unwrap()));
    assert!(tokyo.contains_key(&DayKey::from_raw("2026-10-11")));
    assert!(
        cache
            .daily_tokens(&Calendar::utc())
            .contains_key(&DayKey::from_raw("2026-10-10"))
    );
}

/// Nested transcripts (subagents) are included; hidden dirs are not.
#[test]
fn test_scan_reads_nested_transcripts_and_skips_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    write(
        &projects.join("-Users-me-app/session.jsonl"),
        &format!("{}\n", l("2026-10-10T10:00:00Z", 1)),
    );
    write(
        &projects.join("-Users-me-app/session/subagents/agent.jsonl"),
        &format!("{}\n", l("2026-10-10T10:00:00Z", 2)),
    );
    write(
        &projects.join(".hidden/x.jsonl"),
        &format!("{}\n", l("2026-10-10T10:00:00Z", 3)),
    );
    write(
        &projects.join("notes.JSONL"),
        &format!("{}\n", l("2026-10-10T10:00:00Z", 4)),
    );
    assert_eq!(
        responses(&ClaudeCodeFileCache::new(), &projects, &Calendar::utc()),
        2
    );
}

/// Repeated scans follow partial appends, finishing lines, rewrites and removals.
#[test]
fn test_repeat_scans_follow_appends_rewrites_and_removals() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.jsonl"), dir.path().join("b.jsonl"));
    write(&a, &format!("{}\n", l("2026-10-10T10:00:00Z", 1)));
    write(&b, &format!("{}\n", l("2026-10-10T10:00:00Z", 2)));
    let cache = ClaudeCodeFileCache::new();
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 2);
    let third = l("2026-10-10T10:00:00Z", 3);
    append(&a, &third[..20]);
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 2);
    append(&a, &format!("{}\n", &third[20..]));
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 3);
    write(&a, &format!("{}\n", l("2026-10-10T10:00:00Z", 1)));
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 2);
    fs::remove_file(&b).unwrap();
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 1);
}

/// Each calendar keeps its own running total; a newly asked calendar catches up.
#[test]
fn test_scan_keeps_separate_running_totals_per_calendar() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    write(&p, &format!("{}\n", l("2026-10-10T20:00:00Z", 1)));
    let cache = ClaudeCodeFileCache::new();
    let tokyo = Calendar::fixed(FixedOffset::east_opt(9 * 3600).unwrap());
    cache.update(dir.path());
    assert!(
        cache
            .daily_tokens(&Calendar::utc())
            .contains_key(&DayKey::from_raw("2026-10-10"))
    );
    assert!(
        cache
            .daily_tokens(&tokyo)
            .contains_key(&DayKey::from_raw("2026-10-11"))
    );
    append(&p, &format!("{}\n", l("2026-10-10T21:00:00Z", 2)));
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 2);
    assert_eq!(
        cache.daily_tokens(&tokyo)[&DayKey::from_raw("2026-10-11")]["claude-opus-4-1"]
            .response_count,
        2
    );
    let fresh = cache.daily_tokens(&Calendar::fixed(FixedOffset::east_opt(-3600).unwrap()));
    assert_eq!(
        fresh
            .values()
            .flat_map(|m| m.values())
            .map(|t| t.response_count)
            .sum::<i64>(),
        2
    );
}

/// CRLF transcripts parse, and appends are followed incrementally.
#[test]
fn test_crlf_transcripts_and_appends() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    write(
        &p,
        &format!(
            "{}\r\n{}\r\n",
            l("2026-10-10T10:00:00Z", 1),
            l("2026-10-10T10:00:00Z", 2)
        ),
    );
    let cache = ClaudeCodeFileCache::new();
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 2);
    append(&p, &format!("{}\r\n", l("2026-10-10T10:00:00Z", 3)));
    assert_eq!(responses(&cache, dir.path(), &Calendar::utc()), 3);
}

/// A trailing line cut mid-way, including after a `\r`, waits for the rest.
#[test]
fn test_partial_trailing_lines_wait_for_their_newline() {
    let full = l("2026-10-10T10:00:00Z", 1);
    let data = format!("{full}\r\n{}", &l("2026-10-10T10:00:00Z", 2)[..30]);
    let (entries, consumed) = parse_complete_lines(data.as_bytes());
    assert_eq!(entries.len(), 1);
    assert_eq!(consumed, full.len() + 2);
    let (entries, consumed) = parse_complete_lines(format!("{full}\r").as_bytes());
    assert_eq!((entries.len(), consumed), (1, full.len() + 1));
    assert_eq!(parse_complete_lines(b""), (vec![], 0));
}

/// An unterminated but whole-JSON tail is consumed (parity quirk).
#[test]
fn test_unterminated_whole_json_tail_is_consumed() {
    let full = l("2026-10-10T10:00:00Z", 1);
    let (entries, consumed) = parse_complete_lines(full.as_bytes());
    assert_eq!((entries.len(), consumed), (1, full.len()));
}

/// The usage prefilter splits on `\n` only and keeps `\r` in the line.
#[test]
fn test_usage_line_ranges() {
    let data = b"x\r\n{\"usage\":1}\r\ny\n\"usage\"";
    let r = usage_line_ranges(data);
    assert_eq!(r.len(), 2);
    assert_eq!(&data[r[0].clone()], b"{\"usage\":1}\r");
}

/// No projects directory means Claude Code is not installed.
#[test]
fn test_scan_is_none_without_claude_code() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    assert!(
        scan_daily_tokens(&[missing], &[Calendar::utc()], &ClaudeCodeFileCache::new()).is_none()
    );
    // A file where a directory is expected does not count either.
    let file = dir.path().join("f");
    fs::write(&file, "x").unwrap();
    assert!(scan_daily_tokens(&[file], &[Calendar::utc()], &ClaudeCodeFileCache::new()).is_none());
}

/// `scan_daily_tokens` returns one result per calendar, in order.
#[test]
fn test_scan_returns_one_result_per_calendar() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("a.jsonl"),
        &format!("{}\n", l("2026-10-10T20:00:00Z", 1)),
    );
    let scans = scan_daily_tokens(
        &[dir.path().to_path_buf()],
        &[
            Calendar::utc(),
            Calendar::fixed(FixedOffset::east_opt(9 * 3600).unwrap()),
        ],
        &ClaudeCodeFileCache::new(),
    )
    .unwrap();
    assert!(
        scans[0].contains_key(&DayKey::from_raw("2026-10-10"))
            && scans[1].contains_key(&DayKey::from_raw("2026-10-11"))
    );
}

/// Default candidates are `.config/claude/projects` then `.claude/projects`; `CLAUDE_CONFIG_DIR` overrides.
#[test]
fn test_projects_directories_honors_config_dir() {
    let home = PathBuf::from("/Users/me");
    let env = ClaudeEnvironment::new(&home);
    assert_eq!(
        projects_directories(&env),
        [
            home.join(".config/claude/projects"),
            home.join(".claude/projects")
        ]
    );
    let env = ClaudeEnvironment::new(&home).with_config_dir("/a, /b");
    assert_eq!(
        projects_directories(&env),
        [PathBuf::from("/a/projects"), PathBuf::from("/b/projects")]
    );
    let env = ClaudeEnvironment::new(&home).with_config_dir("  , ,");
    assert!(projects_directories(&env).is_empty());
    let env = ClaudeEnvironment::new(&home).with_config_dir("   ");
    assert_eq!(projects_directories(&env).len(), 2);
}

/// `~` expands to the injected home, for both separators.
#[test]
fn test_config_dir_expands_tilde_to_injected_home() {
    let home = PathBuf::from("/h");
    let env = ClaudeEnvironment::new(&home).with_config_dir("~, ~/x, ~\\y, ~other");
    assert_eq!(
        projects_directories(&env),
        [
            home.join("projects"),
            home.join("x/projects"),
            home.join("y/projects"),
            PathBuf::from("~other/projects")
        ]
    );
}

/// End to end on a fake profile directory: both Windows-style roots are probed, the real home is never touched.
#[test]
fn test_discovery_probes_both_roots_of_an_injected_profile() {
    let profile = tempfile::tempdir().unwrap();
    write(
        &profile.path().join(".claude/projects/p/a.jsonl"),
        &format!("{}\n", l("2026-10-10T10:00:00Z", 1)),
    );
    write(
        &profile.path().join(".config/claude/projects/p/b.jsonl"),
        &format!("{}\n", l("2026-10-10T10:00:00Z", 2)),
    );
    let dirs = projects_directories(&ClaudeEnvironment::new(profile.path()));
    let scans = scan_daily_tokens(&dirs, &[Calendar::utc()], &ClaudeCodeFileCache::new()).unwrap();
    assert_eq!(
        scans[0][&DayKey::from_raw("2026-10-10")]["claude-opus-4-1"].response_count,
        2
    );
}

/// Regression (fuzz `claude_cache_refresh`): a same-size rewrite that still ends in a
/// newline was mistaken for "no new bytes" and kept the old totals.
#[test]
fn test_regression_same_size_rewrite_is_reparsed() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let mk = |day: u8, n: u8| {
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-10-1{day}T10:00:00Z\",\"message\":{{\"model\":\"m\",\"usage\":{{\"input_tokens\":{n}}}}}}}\n"
        )
    };
    write(&p, &mk(2, 2));
    let cache = ClaudeCodeFileCache::new();
    cache.update(dir.path());
    assert!(
        cache
            .daily_tokens(&Calendar::utc())
            .contains_key(&DayKey::from_raw("2026-10-12"))
    );
    write(&p, &mk(1, 4));
    let f = fs::File::options().write(true).open(&p).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    cache.update(dir.path());
    let days = cache.daily_tokens(&Calendar::utc());
    assert!(!days.contains_key(&DayKey::from_raw("2026-10-12")));
    assert_eq!(days[&DayKey::from_raw("2026-10-11")]["m"].input_tokens, 4);
}
