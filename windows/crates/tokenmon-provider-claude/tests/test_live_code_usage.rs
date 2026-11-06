//! The live Claude Code load seam over a temporary home directory.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{FixedOffset, TimeZone, Utc};
use tokenmon_core::ManualClock;
use tokenmon_provider_claude::ledger::{LEDGER_FILE_NAME, decode_ledger};
use tokenmon_provider_claude::{ClaudeEnvironment, DayKey, live_code_usage};

/// The seam reports `None` when Claude Code has no projects directory, and an
/// empty summary when it has one without transcripts.
#[tokio::test]
async fn test_live_code_usage_follows_the_projects_directory() {
    let now = Utc.with_ymd_and_hms(2026, 10, 14, 15, 0, 0).unwrap();
    let home = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();
    let load = live_code_usage(
        ClaudeEnvironment::new(home.path()),
        ledger.path(),
        Arc::new(ManualClock::new(now)),
    );
    assert!(load(now).await.is_none());

    std::fs::create_dir_all(home.path().join(".claude").join("projects")).unwrap();
    let scan = load(now).await.unwrap();
    assert_eq!(scan.summary.all_time.tokens, 0);
    assert!(scan.local_daily_tokens.is_empty());
}

/// Port of `ClaudeCodeUsageLedgerTests::testLedgerDaysAreUTC`: the persisted
/// ledger buckets by UTC whatever the machine's offset, while the local bars
/// use the local day. A 23:30Z entry on a UTC+1 machine lands on 2026-10-10
/// in the ledger file and on 2026-10-11 in the local bars.
#[tokio::test]
async fn test_ledger_days_are_utc_on_a_non_utc_machine() {
    let now = Utc.with_ymd_and_hms(2026, 10, 11, 9, 0, 0).unwrap();
    let home = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();
    let project = home.path().join(".claude").join("projects").join("p");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("s.jsonl"),
        concat!(
            r#"{"type":"assistant","timestamp":"2026-10-10T23:30:00Z","requestId":"req_1","#,
            r#""message":{"id":"msg_1","model":"claude-opus-4-1","usage":{"input_tokens":10,"#,
            r#""output_tokens":20,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            "\n"
        ),
    )
    .unwrap();
    let clock = ManualClock::new(now).with_offset(FixedOffset::east_opt(3600).unwrap());
    let load = live_code_usage(
        ClaudeEnvironment::new(home.path()),
        ledger.path(),
        Arc::new(clock),
    );
    let scan = load(now).await.unwrap();

    let local: Vec<&str> = scan.local_daily_tokens.keys().map(DayKey::as_str).collect();
    assert_eq!(local, ["2026-10-11"]);

    let saved = std::fs::read(ledger.path().join(LEDGER_FILE_NAME)).unwrap();
    let utc: Vec<String> = decode_ledger(&saved)
        .keys()
        .map(|d| d.as_str().to_owned())
        .collect();
    assert_eq!(utc, ["2026-10-10"]);
}
