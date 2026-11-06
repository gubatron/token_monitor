//! Ports of `ClaudeCodeUsageLedgerTests.swift` (spec 03 section 7.1).
#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use tokenmon_provider_claude::ledger::{LEDGER_FILE_NAME, decode_ledger, encode_ledger};
use tokenmon_provider_claude::pricing::priced_model_ids;
use tokenmon_provider_claude::{
    ClaudeCodeUsageLedger, ClaudeCodeUsageSummary, DailyTokens, ModelTokens, UsageTotals,
    base_model_id, cost, display_name, rates,
};

const M: i64 = 1_000_000;

fn day(entries: &[(&str, &[(&str, ModelTokens)])]) -> DailyTokens {
    entries
        .iter()
        .map(|(d, models)| {
            (
                (*d).to_owned(),
                models
                    .iter()
                    .map(|(m, t)| ((*m).to_owned(), *t))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect()
}

fn tok(input: i64, output: i64) -> ModelTokens {
    ModelTokens {
        input_tokens: input,
        output_tokens: output,
        response_count: 1,
        ..Default::default()
    }
}

/// 1M of each of the five kinds on opus-5-5 costs 4+20+0.2+5+8.
#[test]
fn test_cost_uses_each_rate_including_both_cache_lifetimes() {
    let t = ModelTokens {
        input_tokens: M,
        output_tokens: M,
        cache_read_tokens: M,
        cache_write_5m_tokens: M,
        cache_write_1h_tokens: M,
        response_count: 0,
    };
    assert!((cost(&t, "claude-opus-5-5").unwrap() - 37.2).abs() < 1e-9);
}

/// The snapshot date suffix is ignored when looking up rates.
#[test]
fn test_pricing_ignores_snapshot_date_suffix() {
    assert_eq!(
        base_model_id("claude-haiku-4-5-20251001"),
        "claude-haiku-4-5"
    );
    assert_eq!(base_model_id("claude-opus-5-5"), "claude-opus-5-5");
    assert_eq!(
        base_model_id("claude-opus-5-1234567"),
        "claude-opus-5-1234567"
    );
    assert!(rates("claude-haiku-4-5-20251001").is_some());
}

/// Unknown models have no rates and no cost.
#[test]
fn test_unknown_model_has_no_price() {
    assert!(rates("claude-unknown-9").is_none());
    assert!(cost(&tok(1, 1), "claude-unknown-9").is_none());
}

/// Pins every row of the API price table (input, output, cache read, derived writes).
#[test]
fn test_price_table_matches_spec_rows() {
    let want: [(&str, f64, f64, f64, f64, f64); 12] = [
        ("claude-fable-5-1", 10.0, 50.0, 0.25, 12.5, 20.0),
        ("claude-fable-5", 10.0, 50.0, 1.0, 12.5, 20.0),
        ("claude-opus-5-5", 4.0, 20.0, 0.20, 5.0, 8.0),
        ("claude-opus-5", 5.0, 25.0, 0.50, 6.25, 10.0),
        ("claude-opus-4-8", 5.0, 25.0, 0.50, 6.25, 10.0),
        ("claude-opus-4-7", 5.0, 25.0, 0.50, 6.25, 10.0),
        ("claude-opus-4-6", 5.0, 25.0, 0.50, 6.25, 10.0),
        ("claude-sonnet-5-5", 2.0, 10.0, 0.20, 2.5, 4.0),
        ("claude-sonnet-5", 2.0, 10.0, 0.20, 2.5, 4.0),
        ("claude-sonnet-4-6", 3.0, 15.0, 0.30, 3.75, 6.0),
        ("claude-haiku-5-5", 0.10, 0.50, 0.01, 0.125, 0.2),
        ("claude-haiku-4-5", 1.0, 5.0, 0.10, 1.25, 2.0),
    ];
    assert_eq!(priced_model_ids().count(), want.len());
    for (id, i, o, cr, w5, w1) in want {
        let r = rates(id).unwrap_or_else(|| panic!("missing {id}"));
        assert_eq!(
            (
                r.input,
                r.output,
                r.cache_read,
                r.cache_write_5m(),
                r.cache_write_1h()
            ),
            (i, o, cr, w5, w1),
            "{id}"
        );
    }
}

/// Display names follow the OpenCode naming rules after stripping `claude-`.
#[test]
fn test_model_display_names() {
    assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
    assert_eq!(display_name("claude-fable-5-1"), "Fable 5.1");
    assert_eq!(display_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
    assert_eq!(display_name(""), "");
}

/// Saved days survive; days in both keep the larger counts per field.
#[test]
fn test_merge_keeps_the_larger_counts_and_days_no_longer_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = ClaudeCodeUsageLedger::new(dir.path());
    ledger.merge(&day(&[
        ("2026-09-01", &[("claude-opus-5-5", tok(50, 0))]),
        ("2026-10-01", &[("claude-opus-5-5", tok(10, 90))]),
    ]));
    let merged = ledger.merge(&day(&[(
        "2026-10-01",
        &[
            ("claude-opus-5-5", tok(30, 40)),
            ("claude-sonnet-5", tok(5, 0)),
        ],
    )]));
    assert_eq!(merged["2026-09-01"]["claude-opus-5-5"].input_tokens, 50);
    assert_eq!(merged["2026-10-01"]["claude-opus-5-5"].input_tokens, 30);
    assert_eq!(merged["2026-10-01"]["claude-opus-5-5"].output_tokens, 90);
    assert_eq!(merged["2026-10-01"]["claude-sonnet-5"].input_tokens, 5);
}

/// A new ledger on the same directory still has days whose transcripts are gone.
#[test]
fn test_ledger_keeps_days_after_their_transcripts_are_deleted() {
    let dir = tempfile::tempdir().unwrap();
    ClaudeCodeUsageLedger::new(dir.path())
        .merge(&day(&[("2026-09-01", &[("claude-opus-5-5", tok(1, 1))])]));
    let b = ClaudeCodeUsageLedger::new(dir.path())
        .merge(&day(&[("2026-10-01", &[("claude-opus-5-5", tok(2, 2))])]));
    assert!(b.contains_key("2026-09-01") && b.contains_key("2026-10-01"));
}

/// A version 1 file is discarded.
#[test]
fn test_ledger_discards_an_earlier_format() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(LEDGER_FILE_NAME),
        br#"{"version":1,"days":{"2026-10-01":{"claude-opus-5-5":{"inputTokens":9}}}}"#,
    )
    .unwrap();
    let merged = ClaudeCodeUsageLedger::new(dir.path())
        .merge(&day(&[("2026-10-02", &[("claude-opus-5-5", tok(1, 1))])]));
    assert!(!merged.contains_key("2026-10-01") && merged.contains_key("2026-10-02"));
}

/// An unreadable file starts the ledger empty.
#[test]
fn test_ledger_starts_empty_when_the_saved_file_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(LEDGER_FILE_NAME), b"not json").unwrap();
    let merged = ClaudeCodeUsageLedger::new(dir.path())
        .merge(&day(&[("2026-10-02", &[("claude-opus-5-5", tok(1, 1))])]));
    assert_eq!(merged.len(), 1);
}

/// All six count fields are required, so a payload missing one is empty.
#[test]
fn test_ledger_payload_requires_every_count_field() {
    let bad = br#"{"days":{"d":{"m":{"inputTokens":1}}},"version":2}"#;
    assert!(decode_ledger(bad).is_empty());
}

/// The payload is compact with sorted keys, matching the macOS file.
#[test]
fn test_ledger_encoding_is_sorted_compact_json() {
    let days = day(&[("2026-10-01", &[("claude-opus-5-5", tok(1, 2))])]);
    let s = String::from_utf8(encode_ledger(&days)).unwrap();
    assert_eq!(
        s,
        r#"{"days":{"2026-10-01":{"claude-opus-5-5":{"cacheReadTokens":0,"cacheWrite1hTokens":0,"cacheWrite5mTokens":0,"inputTokens":1,"outputTokens":2,"responseCount":1}}},"version":2}"#
    );
    assert_eq!(decode_ledger(s.as_bytes()), days);
}

/// The file is only rewritten when the merge changed something.
#[test]
fn test_merge_writes_only_when_changed() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = ClaudeCodeUsageLedger::new(dir.path());
    let d = day(&[("2026-10-01", &[("claude-opus-5-5", tok(1, 1))])]);
    ledger.merge(&d);
    let path = dir.path().join(LEDGER_FILE_NAME);
    std::fs::remove_file(&path).unwrap();
    ledger.merge(&d);
    assert!(!path.exists(), "unchanged merge must not rewrite");
}

/// Spend label marks unpriced usage with a plus.
#[test]
fn test_spend_marks_left_out_unpriced_usage() {
    let t = |u| UsageTotals {
        tokens: 0,
        cost_usd: 12.5,
        has_unpriced: u,
    };
    assert_eq!(t(false).spend_label(), "$12.50");
    assert_eq!(t(true).spend_label(), "$12.50+");
}

/// Month splits from all-time and models rank by value.
#[test]
fn test_summary_splits_month_from_all_time_and_ranks_models_by_value() {
    let out = |n| tok(0, n);
    let days = day(&[
        ("2026-09-30", &[("claude-opus-5-5", out(M))]),
        (
            "2026-10-01",
            &[("claude-opus-5-5", out(M)), ("claude-sonnet-5", out(M))],
        ),
        ("2026-10-22", &[("claude-sonnet-5", out(M))]),
    ]);
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let s = ClaudeCodeUsageSummary::make(&days, now);
    assert_eq!(s.month.tokens, 3 * M);
    assert!((s.month.cost_usd - 40.0).abs() < 1e-9);
    assert_eq!(s.all_time.tokens, 4 * M);
    assert!((s.all_time.cost_usd - 60.0).abs() < 1e-9);
    let ids: Vec<_> = s.month_models.iter().map(|r| r.model_id.as_str()).collect();
    assert_eq!(ids, ["claude-sonnet-5", "claude-opus-5-5"]);
    assert!(
        s.month_models
            .iter()
            .all(|r| (r.share_percent - 50.0).abs() < 1e-9)
    );
    assert!((s.month_models[0].cost_usd - 20.0).abs() < 1e-9);
}

/// Unpriced models are flagged, have share 0 and leave the rest at 100.
#[test]
fn test_summary_flags_unpriced_models() {
    let days = day(&[(
        "2026-10-01",
        &[
            ("claude-opus-5-5", tok(0, M)),
            ("claude-mystery", tok(0, 500)),
        ],
    )]);
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    let s = ClaudeCodeUsageSummary::make(&days, now);
    assert!(s.month.has_unpriced);
    let mystery = s
        .month_models
        .iter()
        .find(|r| r.model_id == "claude-mystery")
        .unwrap();
    assert!(mystery.is_unpriced);
    assert_eq!(mystery.share_percent, 0.0);
    assert!((s.month_models[0].share_percent - 100.0).abs() < 1e-9);
}

/// At 00:30 UTC on the 1st the new month is empty while all-time keeps data.
#[test]
fn test_summary_is_empty_at_the_start_of_a_month() {
    let days = day(&[("2026-09-30", &[("claude-opus-5-5", tok(5, 5))])]);
    let now = Utc.with_ymd_and_hms(2026, 10, 1, 0, 30, 0).unwrap();
    let s = ClaudeCodeUsageSummary::make(&days, now);
    assert_eq!(s.month.tokens, 0);
    assert!(s.month_models.is_empty());
    assert_eq!(s.all_time.tokens, 10);
}

/// en_US formatting groups thousands and rounds to cents.
#[test]
fn test_format_usd_groups_thousands() {
    use tokenmon_provider_claude::summary::format_usd;
    assert_eq!(format_usd(1234.567), "$1,234.57");
    assert_eq!(format_usd(0.0), "$0.00");
    assert_eq!(format_usd(f64::NAN), "$0.00");
}
