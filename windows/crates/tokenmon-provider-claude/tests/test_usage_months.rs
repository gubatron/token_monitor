//! This month's and last month's per-model Claude Code totals from the UTC
//! ledger, how much of last month the ledger covers (Swift
//! `ClaudeUsageMonthTests`), and the hover label's strings with the row's
//! screen-reader label (Swift `ClaudeModelHoverDetailTests`).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use tokenmon_core::usage::{Calendar, DayKey, HoverLabelRow};
use tokenmon_provider_claude::hover::{ClaudeModelHoverDetail, hover_usd};
use tokenmon_provider_claude::month::{ClaudeMonthCoverage, ClaudeUsageMonth};
use tokenmon_provider_claude::{ClaudeCodeUsageSummary, DailyTokens, ModelTokens};

const OPUS: &str = "claude-opus-5-5";

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

fn instant(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .unwrap()
        .with_timezone(&Utc)
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn tokens(input: i64, output: i64) -> ModelTokens {
    ModelTokens {
        input_tokens: input,
        output_tokens: output,
        response_count: 1,
        ..ModelTokens::default()
    }
}

/// A ledger from `(day key, [(model id, tokens)])` entries.
fn ledger(entries: &[(&str, &[(&str, ModelTokens)])]) -> DailyTokens {
    entries
        .iter()
        .map(|(key, models)| {
            let by_model: BTreeMap<String, ModelTokens> =
                models.iter().map(|(m, t)| ((*m).to_owned(), *t)).collect();
            (DayKey::from_raw(*key), by_model)
        })
        .collect()
}

// ---- Months ----------------------------------------------------------------

/// Start, previous month, key prefix and membership of a UTC month.
#[test]
fn test_month_start_previous_and_keys() {
    let month = ClaudeUsageMonth::containing(instant("2026-11-09T15:00:00Z"), utc());
    assert_eq!(month.first_day(), date(2026, 11, 1));
    assert_eq!(month.key_prefix(), "2026-11");
    assert_eq!(month.first_day_key(), "2026-11-01");
    assert_eq!(month.name(), "November");
    assert_eq!(month.previous().first_day(), date(2026, 10, 1));
    assert_eq!(month.previous().name(), "October");
    assert!(month.contains_day_key(&DayKey::from_raw("2026-11-30")));
    assert!(!month.contains_day_key(&DayKey::from_raw("2026-10-31")));
    assert!(!month.contains_day_key(&DayKey::from_raw("2025-11-09")));
}

/// January's previous month is last year's December.
#[test]
fn test_previous_month_crosses_the_year() {
    let january = ClaudeUsageMonth::containing(instant("2027-01-01T00:00:00Z"), utc());
    assert_eq!(january.previous().key_prefix(), "2026-12");
    assert_eq!(january.previous().name(), "December");
}

/// Names and short dates come from the month's own calendar.
#[test]
fn test_names_use_the_months_calendar_not_the_system_zone() {
    let month = ClaudeUsageMonth::containing(instant("2026-11-01T00:00:00Z"), utc());
    assert_eq!(month.name(), "November");
    assert_eq!(month.short_date(date(2026, 11, 1)), "Nov 1");
}

/// A span runs from a day to the month's end; the last day stands alone.
#[test]
fn test_span_runs_from_a_day_to_the_month_end() {
    let october = ClaudeUsageMonth::containing(instant("2026-10-15T00:00:00Z"), utc());
    assert_eq!(october.span(date(2026, 10, 9)), "Oct 9–31");
    assert_eq!(october.span(date(2026, 10, 31)), "Oct 31");
    let february = ClaudeUsageMonth::containing(instant("2028-02-10T00:00:00Z"), utc());
    assert_eq!(february.span(date(2028, 2, 10)), "Feb 10–29");
}

// ---- Aggregation -------------------------------------------------------------

/// Last month sums each model across its days; this month stays separate.
#[test]
fn test_last_month_sums_each_model_across_its_days() {
    let days = ledger(&[
        (
            "2026-10-01",
            &[(OPUS, tokens(10, 0)), ("claude-sonnet-5-5", tokens(0, 5))],
        ),
        ("2026-10-31", &[(OPUS, tokens(20, 7))]),
        ("2026-11-01", &[(OPUS, tokens(1_000, 0))]),
    ]);
    let s = ClaudeCodeUsageSummary::make(&days, instant("2026-11-09T12:00:00Z"));
    assert_eq!(s.current_month.key_prefix(), "2026-11");
    assert_eq!(s.previous_month.key_prefix(), "2026-10");
    let prev = &s.previous_month_models;
    assert_eq!(
        prev[OPUS],
        ModelTokens {
            input_tokens: 30,
            output_tokens: 7,
            response_count: 2,
            ..ModelTokens::default()
        }
    );
    assert_eq!(prev["claude-sonnet-5-5"].output_tokens, 5);
    let ids: Vec<&str> = s.month_models.iter().map(|m| m.model_id.as_str()).collect();
    assert_eq!(ids, [OPUS]);
    assert_eq!(s.month_models[0].breakdown.input_tokens, 1_000);
}

/// The first and last UTC days belong to their own months; two months back is
/// left out.
#[test]
fn test_month_boundaries_follow_utc_days() {
    let days = ledger(&[
        ("2026-09-30", &[(OPUS, tokens(1, 0))]),
        ("2026-10-01", &[(OPUS, tokens(10, 0))]),
        ("2026-10-31", &[(OPUS, tokens(100, 0))]),
        ("2026-11-01", &[(OPUS, tokens(1_000, 0))]),
    ]);
    let s = ClaudeCodeUsageSummary::make(&days, instant("2026-11-01T00:00:00Z"));
    assert_eq!(s.previous_month_models[OPUS].input_tokens, 110);
    assert_eq!(s.month.tokens, 1_000);
}

/// At 17:00 on October 31 in Los Angeles it is already November 1 in UTC; the
/// ledger's calendar decides the month.
#[test]
fn test_the_ledger_calendar_decides_the_month_not_the_local_zone() {
    let days = ledger(&[
        ("2026-10-31", &[(OPUS, tokens(100, 0))]),
        ("2026-11-01", &[(OPUS, tokens(1_000, 0))]),
    ]);
    let now = instant("2026-10-31T17:00:00-07:00");

    let in_utc = ClaudeCodeUsageSummary::make(&days, now);
    assert_eq!(in_utc.current_month.name(), "November");
    assert_eq!(in_utc.month.tokens, 1_000);
    assert_eq!(in_utc.previous_month_models[OPUS].input_tokens, 100);

    // Reckoned in a local calendar instead, the same instant is October.
    let la = Calendar::fixed(FixedOffset::west_opt(7 * 3600).unwrap());
    let local = ClaudeCodeUsageSummary::make_in(&days, now, &la);
    assert_eq!(local.current_month.name(), "October");
    assert_eq!(local.month.tokens, 100);
    assert!(!local.previous_month_models.contains_key(OPUS));
}

/// A model used in only one of the two months.
#[test]
fn test_models_missing_from_one_month() {
    let days = ledger(&[
        ("2026-10-05", &[("claude-haiku-5-5", tokens(50, 0))]),
        ("2026-11-02", &[("claude-fable-5-1", tokens(0, 30))]),
    ]);
    let s = ClaudeCodeUsageSummary::make(&days, instant("2026-11-09T00:00:00Z"));
    assert!(!s.previous_month_models.contains_key("claude-fable-5-1"));
    assert_eq!(s.previous_month_models["claude-haiku-5-5"].input_tokens, 50);
    let ids: Vec<&str> = s.month_models.iter().map(|m| m.model_id.as_str()).collect();
    assert_eq!(ids, ["claude-fable-5-1"]);
}

/// All-zero counts last month count as unused.
#[test]
fn test_zero_token_days_do_not_count_as_usage() {
    let zero = ModelTokens {
        response_count: 1,
        ..ModelTokens::default()
    };
    let days = ledger(&[
        ("2026-10-05", &[(OPUS, zero)]),
        ("2026-11-02", &[(OPUS, tokens(0, 30))]),
    ]);
    let s = ClaudeCodeUsageSummary::make(&days, instant("2026-11-09T00:00:00Z"));
    assert!(s.previous_month_models.is_empty());
}

// ---- Coverage -----------------------------------------------------------------

fn key(s: &str) -> DayKey {
    DayKey::from_raw(s)
}

/// A ledger reaching the month's first day (or earlier) covers it whole.
#[test]
fn test_coverage_is_complete_when_the_ledger_reaches_the_month_start() {
    let november = ClaudeUsageMonth::containing(instant("2026-11-09T00:00:00Z"), utc());
    let october = november.previous();
    let c = |k: &str| ClaudeMonthCoverage::of(october, Some(&key(k)));
    assert_eq!(c("2026-10-01"), ClaudeMonthCoverage::Complete);
    assert_eq!(c("2026-09-12"), ClaudeMonthCoverage::Complete);
}

/// A ledger starting inside the month covers it only from that day.
#[test]
fn test_coverage_is_partial_when_the_ledger_starts_inside_the_month() {
    let october = ClaudeUsageMonth::containing(instant("2026-10-15T00:00:00Z"), utc());
    assert_eq!(
        ClaudeMonthCoverage::of(october, Some(&key("2026-10-09"))),
        ClaudeMonthCoverage::Partial {
            since: date(2026, 10, 9)
        }
    );
}

/// A ledger starting after the month, or empty, gives no coverage.
#[test]
fn test_coverage_is_none_when_the_ledger_starts_after_the_month() {
    let october = ClaudeUsageMonth::containing(instant("2026-10-15T00:00:00Z"), utc());
    assert_eq!(
        ClaudeMonthCoverage::of(october, Some(&key("2026-11-04"))),
        ClaudeMonthCoverage::None {
            recorded_since: Some(date(2026, 11, 4))
        }
    );
    assert_eq!(
        ClaudeMonthCoverage::of(october, None),
        ClaudeMonthCoverage::None {
            recorded_since: None
        }
    );
}

/// The summary reads coverage from the earliest saved day.
#[test]
fn test_summary_reports_last_months_coverage_from_the_earliest_saved_day() {
    let now = instant("2026-11-09T00:00:00Z");
    let partial = ClaudeCodeUsageSummary::make(
        &ledger(&[
            ("2026-10-20", &[(OPUS, tokens(1, 0))]),
            ("2026-11-02", &[(OPUS, tokens(1, 0))]),
        ]),
        now,
    );
    assert_eq!(
        partial.previous_month_coverage,
        ClaudeMonthCoverage::Partial {
            since: date(2026, 10, 20)
        }
    );
    let complete = ClaudeCodeUsageSummary::make(
        &ledger(&[
            ("2026-09-28", &[(OPUS, tokens(1, 0))]),
            ("2026-11-02", &[(OPUS, tokens(1, 0))]),
        ]),
        now,
    );
    assert_eq!(
        complete.previous_month_coverage,
        ClaudeMonthCoverage::Complete
    );
}

// ---- Hover detail ---------------------------------------------------------------

/// Opus 5.5 output tokens worth `usd` at $20 per million.
fn opus_output(usd: f64) -> ModelTokens {
    ModelTokens {
        output_tokens: (usd / 20.0 * 1_000_000.0) as i64,
        response_count: 1,
        ..ModelTokens::default()
    }
}

/// The hover detail of `model_id` in a summary of `days`, made on 2026-11-09.
fn detail(model_id: &str, days: &DailyTokens) -> ClaudeModelHoverDetail {
    let s = ClaudeCodeUsageSummary::make(days, instant("2026-11-09T12:00:00Z"));
    let model = s
        .month_models
        .iter()
        .find(|m| m.model_id == model_id)
        .expect("model in this month");
    ClaudeModelHoverDetail::new(model, &s)
}

fn row(label: &str, value: &str) -> HoverLabelRow {
    HoverLabelRow {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

/// Title, total, value and breakdown of this month.
#[test]
fn test_title_total_value_and_breakdown() {
    let counts = ModelTokens {
        input_tokens: 1_200_000,
        output_tokens: 2_000_000,
        cache_read_tokens: 30_000_000,
        cache_write_5m_tokens: 400_000,
        cache_write_1h_tokens: 100_000,
        response_count: 9,
    };
    let d = detail(OPUS, &ledger(&[("2026-11-02", &[(OPUS, counts)])]));
    assert_eq!(d.title, "Opus 5.5 · November");
    assert_eq!(d.tokens, 33_700_000);
    assert_eq!(d.value_text, "≈ $53.60");
    assert_eq!(
        d.breakdown,
        vec![
            row("Input", "1.2M"),
            row("Output", "2.0M"),
            row("Cache read", "30M"),
            row("Cache write", "500K"),
        ]
    );
}

/// A spend under a cent is not shown as zero.
#[test]
fn test_spend_under_a_cent_is_not_shown_as_zero() {
    let haiku = "claude-haiku-5-5";
    let d = detail(
        haiku,
        &ledger(&[("2026-11-02", &[(haiku, tokens(10_000, 0))])]),
    );
    assert_eq!(d.value_text, "≈ <$0.01");
    assert_eq!(hover_usd(0.0), "$0.00");
    assert_eq!(hover_usd(1_712.45), "$1,712.45");
}

/// A rise against a fully recorded last month.
#[test]
fn test_increase_against_last_month() {
    let d = detail(
        OPUS,
        &ledger(&[
            ("2026-10-01", &[(OPUS, opus_output(800.0))]),
            ("2026-11-02", &[(OPUS, opus_output(1_000.0))]),
        ]),
    );
    assert_eq!(
        d.previous_line.as_deref(),
        Some("October: 40M tokens · ≈ $800.00")
    );
    assert_eq!(d.change_line, "↑ 25% · +$200.00 vs October");
}

/// A drop uses a true minus sign.
#[test]
fn test_decrease_against_last_month() {
    let d = detail(
        OPUS,
        &ledger(&[
            ("2026-10-01", &[(OPUS, opus_output(1_000.0))]),
            ("2026-11-02", &[(OPUS, opus_output(600.0))]),
        ]),
    );
    assert_eq!(
        d.previous_line.as_deref(),
        Some("October: 50M tokens · ≈ $1,000.00")
    );
    assert_eq!(d.change_line, "↓ 40% · \u{2212}$400.00 vs October");
}

/// The same spend reads "Same as October".
#[test]
fn test_unchanged_against_last_month() {
    let d = detail(
        OPUS,
        &ledger(&[
            ("2026-10-01", &[(OPUS, opus_output(100.0))]),
            ("2026-11-02", &[(OPUS, opus_output(100.0))]),
        ]),
    );
    assert_eq!(d.change_line, "Same as October");
}

/// With the ledger reaching before October, an unused model is really new.
#[test]
fn test_model_unused_last_month_is_new() {
    let fable = "claude-fable-5-1";
    let d = detail(
        fable,
        &ledger(&[
            ("2026-09-20", &[(OPUS, opus_output(5.0))]),
            ("2026-11-02", &[(fable, tokens(0, 78_200))]),
        ]),
    );
    assert_eq!(d.previous_line, None);
    assert_eq!(d.change_line, "New this month");
}

/// Last month's few tokens priced at $0.00 give a rise without a percent.
#[test]
fn test_rise_from_zero_spend_last_month() {
    let haiku = "claude-haiku-5-5";
    let d = detail(
        haiku,
        &ledger(&[
            ("2026-10-01", &[(haiku, tokens(100, 0))]),
            ("2026-11-02", &[(haiku, tokens(0, 4_000_000))]),
        ]),
    );
    assert_eq!(
        d.previous_line.as_deref(),
        Some("October: 100 tokens · ≈ <$0.01")
    );
    assert_eq!(d.change_line, "↑ $2.00 vs October");
}

/// An unpriced model compares tokens.
#[test]
fn test_unpriced_model_compares_tokens() {
    let unknown = "claude-unknown-9";
    let d = detail(
        unknown,
        &ledger(&[
            ("2026-10-01", &[(unknown, tokens(0, 1_000_000))]),
            ("2026-11-02", &[(unknown, tokens(0, 1_500_000))]),
        ]),
    );
    assert_eq!(d.value_text, "No price");
    assert_eq!(d.previous_line.as_deref(), Some("October: 1.0M tokens"));
    assert_eq!(d.change_line, "↑ 50% · +500K tokens vs October");
}

/// A record starting on October 9 names last month by the days it covers.
#[test]
fn test_partly_recorded_month_is_named_by_its_days() {
    let d = detail(
        OPUS,
        &ledger(&[
            ("2026-10-09", &[(OPUS, opus_output(800.0))]),
            ("2026-11-02", &[(OPUS, opus_output(1_000.0))]),
        ]),
    );
    assert_eq!(
        d.previous_line.as_deref(),
        Some("Oct 9–31: 40M tokens · ≈ $800.00")
    );
    assert_eq!(d.change_line, "↑ 25% · +$200.00 vs Oct 9–31");
}

/// A model missing from a partly recorded month is not called new.
#[test]
fn test_model_missing_from_a_partly_recorded_month_is_not_new() {
    let fable = "claude-fable-5-1";
    let d = detail(
        fable,
        &ledger(&[
            ("2026-10-09", &[(OPUS, opus_output(5.0))]),
            ("2026-11-02", &[(fable, tokens(0, 78_200))]),
        ]),
    );
    assert_eq!(d.previous_line, None);
    assert_eq!(d.change_line, "No usage recorded Oct 9–31");
}

/// With nothing recorded for last month there is no comparison.
#[test]
fn test_unrecorded_last_month_gives_no_comparison() {
    let d = detail(
        OPUS,
        &ledger(&[("2026-11-04", &[(OPUS, opus_output(1_000.0))])]),
    );
    assert_eq!(d.previous_line, None);
    assert_eq!(d.change_line, "No October history (recorded since Nov 4)");
}

/// The screen-reader label speaks the whole hover label.
#[test]
fn test_accessibility_label_speaks_the_whole_label() {
    let d = detail(
        OPUS,
        &ledger(&[
            ("2026-10-01", &[(OPUS, opus_output(800.0))]),
            ("2026-11-02", &[(OPUS, opus_output(1_000.0))]),
        ]),
    );
    assert_eq!(
        d.accessibility_label,
        "Opus 5.5, November: 50M tokens, about $1,000.00. \
         Input 0, Output 50M, Cache read 0, Cache write 0. \
         October: 40M tokens, about $800.00. \
         Up 25%, $200.00 more than October"
    );
}

/// Without a previous-month line the label says so and still reads the rest.
#[test]
fn test_accessibility_label_without_a_comparison() {
    let unknown = "claude-unknown-9";
    let d = detail(
        unknown,
        &ledger(&[("2026-11-04", &[(unknown, tokens(0, 1_000))])]),
    );
    assert_eq!(
        d.accessibility_label,
        "Unknown 9, November: 1K tokens, no listed price. \
         Input 0, Output 1K, Cache read 0, Cache write 0. \
         No October history (recorded since Nov 4)"
    );
}
