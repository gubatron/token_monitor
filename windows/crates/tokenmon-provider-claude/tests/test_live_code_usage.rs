//! The live Claude Code load seam over a temporary home directory.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tokenmon_core::ManualClock;
use tokenmon_provider_claude::{ClaudeEnvironment, live_code_usage};

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
