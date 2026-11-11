//! Port of Swift `ThisMacLabelTests`: which panel sections carry the "This PC"
//! label (issue #85). Figures read from this computer's logs must say so;
//! account-wide figures must not.
//!
//! The section tests read the grid and header each panel builder actually
//! produces, so they fail when a local section stops passing its source
//! along, not only when a constant changes.
#![allow(missing_docs, clippy::unwrap_used)]

mod panels_common;

use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use panels_common::*;
use tokenmon_core::Srgb;
use tokenmon_core::usage::{DayKey, UsageSource};
use tokenmon_provider_claude::budget::daily_token_usage;
use tokenmon_provider_claude::{ClaudeCodeUsageSummary, ModelTokens};
use tokenmon_ui::panels::claude_chatgpt::*;
use tokenmon_ui::panels::common::*;
use tokenmon_ui::panels::opencode_openrouter::*;

const OPUS: &str = "claude-opus-5-5";

/// A Claude Code section as the panel builder gets it from the summary.
fn claude_code() -> ClaudeCodeInput {
    let days = BTreeMap::from([(
        DayKey::from_raw("2026-11-02"),
        BTreeMap::from([(
            OPUS.to_owned(),
            ModelTokens {
                input_tokens: 1_200_000,
                output_tokens: 2_000_000,
                response_count: 3,
                ..ModelTokens::default()
            },
        )]),
    )]);
    let now = Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap();
    let summary = ClaudeCodeUsageSummary::make(&days, now);
    ClaudeCodeInput {
        month: CodeTotalsInput {
            tokens: 3_200_000,
            cost_usd: 12.5,
            has_unpriced: false,
        },
        all_time: CodeTotalsInput {
            tokens: 3_200_000,
            cost_usd: 12.5,
            has_unpriced: false,
        },
        month_models: summary
            .month_models
            .iter()
            .map(|m| CodeModelInput {
                name: "Opus 5.5".to_owned(),
                is_unpriced: false,
                cost_usd: m.cost_usd,
                share_percent: 100.0,
                hover: None,
            })
            .collect(),
        models_expanded: true,
    }
}

fn opencode_snapshot() -> OpenCodeSnapshotInput {
    OpenCodeSnapshotInput {
        pool_name: "Mixed".into(),
        windows: Vec::new(),
        is_estimated: false,
        daily: None,
        models: vec![OpenCodeModelInput {
            name: "Kimi".into(),
            cost_usd: 1.5,
            is_cost_estimated: false,
            is_cost_unpriced: false,
            percent_of_window: 50.0,
            color: Srgb::rgb(0.5, 0.5, 0.5),
        }],
        show_all_models: false,
        models_expanded: true,
        local_stats: Some(OpenCodeLocalStatsInput {
            monthly_usd: 12.0,
            tokens: 1_000,
            input_tokens: 400,
            output_tokens: 600,
        }),
    }
}

fn opencode_usage() -> OpenCodeUsage {
    build_opencode(
        &OpenCodeInput {
            status: status(),
            snapshot: Some(opencode_snapshot()),
        },
        &ctx(),
    )
    .usage
    .unwrap()
}

/// Each cell's spoken title, in order.
fn spoken_titles(grid: &MetricGridVm) -> Vec<String> {
    grid.stats.iter().map(|s| grid.spoken_title(s)).collect()
}

// ------------------------------ Data sources --------------------------------

/// Claude Code's and OpenCode's local data are this-computer sources.
#[test]
fn test_local_sources_are_this_computer() {
    assert_eq!(ClaudeCodeUsageSummary::SOURCE, UsageSource::ThisComputer);
    assert_eq!(OPENCODE_STATS_SOURCE, UsageSource::ThisComputer);
    assert_eq!(OPENCODE_MODELS_SOURCE, UsageSource::ThisComputer);
    assert_eq!(CLAUDE_CODE_SOURCE, UsageSource::ThisComputer);
    let models = BTreeMap::from([(
        OPUS.to_owned(),
        ModelTokens {
            output_tokens: 1,
            ..ModelTokens::default()
        },
    )]);
    assert_eq!(daily_token_usage(&models).source, UsageSource::ThisComputer);
    // Cursor's server events are the account's.
    assert_eq!(
        tokenmon_provider_cursor::aggregate::CURSOR_SOURCE,
        UsageSource::Account
    );
}

// ------------------------------ Claude panel --------------------------------

/// The Claude Code stats card carries the tag and speaks "on this PC".
#[test]
fn test_claude_code_stats_are_labelled() {
    let stats = build_code_section(&claude_code()).metrics;
    assert_eq!(stats.source, UsageSource::ThisComputer);
    assert_eq!(stats.tag().as_deref(), Some("This PC"));
    assert_eq!(
        spoken_titles(&stats),
        [
            "Tokens this month on this PC",
            "Spend this month on this PC",
            "Total tokens on this PC",
            "Total spend on this PC",
        ]
    );
}

/// The Claude models header carries the tag and speaks "on this PC".
#[test]
fn test_claude_models_header_is_labelled() {
    let header = build_code_section(&claude_code()).models.unwrap();
    assert_eq!(header.source.tag().as_deref(), Some("This PC"));
    assert_eq!(header.spoken_title(), "Models This Month on this PC");
}

// ----------------------------- OpenCode panel -------------------------------

/// The OpenCode local stats card carries the tag.
#[test]
fn test_opencode_local_stats_are_labelled() {
    let grid = opencode_usage().metrics.unwrap();
    assert_eq!(grid.tag().as_deref(), Some("This PC"));
    assert_eq!(
        spoken_titles(&grid),
        [
            "Monthly spend on this PC",
            "Total tokens on this PC",
            "Input tokens on this PC",
            "Output tokens on this PC",
        ]
    );
}

/// The OpenCode Top Models header carries the tag.
#[test]
fn test_opencode_models_header_is_labelled() {
    let header = opencode_usage().models.unwrap();
    assert_eq!(header.source.tag().as_deref(), Some("This PC"));
    assert_eq!(header.spoken_title(), "Top Models by Spend on this PC");
}

// ---------------------------- Account figures -------------------------------

/// Account-wide stats and headers carry no tag and no location.
#[test]
fn test_account_stats_are_not_labelled() {
    let grid = MetricGridVm::new(
        UsageSource::Account,
        vec![MetricVm::new("Monthly spend", "$1.00".into())],
    );
    assert_eq!(grid.tag(), None);
    assert_eq!(spoken_titles(&grid), ["Monthly spend"]);
    let header = ModelsCardVm::new("Models", UsageSource::Account, true, true, Vec::new(), true);
    assert_eq!(header.source.tag(), None);
    assert_eq!(header.spoken_title(), "Models");
}
