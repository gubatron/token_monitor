//! The daily bars' hover label from real provider data: each provider's
//! per-day tokens travel from its tally through the app's chart input into the
//! panel view model, and the label strings are what the Mac shows (date line,
//! % of the pool and of the daily allowance, token total with `≈ $`, the
//! input/output/cache breakdown, top models).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_app::daily::{ChartContext, daily_input};
use tokenmon_core::ProviderId;
use tokenmon_core::usage::{
    Calendar, DailyBudgetDay, DailyTokenUsageTally, DayKey, TokenCounts, attaching_tokens_map,
};
use tokenmon_provider_claude::budget::attach_tokens;
use tokenmon_provider_claude::{DailyTokens, ModelTokens};
use tokenmon_ui::panels::common::{AllowanceNoun, DailyBudgetVm, PanelContext, build_daily_budget};

fn utc(m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, m, d, 0, 0, 0).unwrap()
}

/// Seven bars, Mon Nov 2 .. Sun Nov 8; Tuesday spent 14.3% of the weekly pool.
fn week_bars() -> Vec<DailyBudgetDay> {
    (2..=8)
        .map(|d| {
            let spent = if d == 3 { 14.3 } else { 0.0 };
            DailyBudgetDay::new(utc(11, d), spent, 100.0 / 7.0)
        })
        .collect()
}

fn chart(provider: ProviderId, noun: AllowanceNoun) -> (Calendar<'static>, ChartContext<'static>) {
    let cal = Calendar::utc();
    let ctx = ChartContext {
        provider,
        noun,
        period_used_percent: None,
        period_start: None,
        resets_at: None,
        week_offset: 0,
        now: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
        // Leaked once per test: the context borrows its calendar.
        calendar: Box::leak(Box::new(cal)),
    };
    (cal, ctx)
}

/// The panel's daily card for `days` as `provider` would show it.
fn card(days: &[DailyBudgetDay], provider: ProviderId, noun: AllowanceNoun) -> DailyBudgetVm {
    let (cal, ctx) = chart(provider, noun);
    let input = daily_input(days, &ctx).expect("bars");
    let panel = PanelContext {
        now: ctx.now,
        calendar: cal,
    };
    build_daily_budget(&input, &panel).expect("card")
}

fn pair(l: &str, v: &str) -> (String, String) {
    (l.to_owned(), v.to_owned())
}

/// Claude: Claude Code transcripts (local days) become the Tuesday label.
#[test]
fn test_claude_bar_hover_shows_tokens_dollars_and_top_models() {
    let opus = ModelTokens {
        input_tokens: 120_000,
        output_tokens: 1_500_000,
        cache_read_tokens: 30_000_000,
        cache_write_5m_tokens: 900_000,
        cache_write_1h_tokens: 100_000,
        response_count: 40,
    };
    let sonnet = ModelTokens {
        input_tokens: 20_000,
        output_tokens: 400_000,
        cache_read_tokens: 8_000_000,
        response_count: 12,
        ..ModelTokens::default()
    };
    let local: DailyTokens = BTreeMap::from([(
        DayKey::from_raw("2026-11-03"),
        BTreeMap::from([
            ("claude-opus-5-5".to_owned(), opus),
            ("claude-sonnet-5-5".to_owned(), sonnet),
        ]),
    )]);
    let days = attach_tokens(week_bars(), &local, &Calendar::utc());
    let vm = card(&days, ProviderId::Claude, AllowanceNoun::Weekly);

    let tuesday = &vm.bars[1].hover;
    assert_eq!(tuesday.title, "Tuesday, Nov 3");
    assert_eq!(
        tuesday.usage_line.as_deref(),
        Some("14.3% of weekly · 100% of daily allowance")
    );
    let t = tuesday.tokens.as_ref().expect("token block");
    assert_eq!(t.total_text, "41M");
    // API-rate value of both models; every model is priced, so no `+`.
    let cost = t.cost_text.as_deref().expect("dollars");
    assert!(cost.starts_with("≈ $") && !cost.ends_with('+'), "{cost}");
    assert_eq!(cost, "≈ $47.42");
    assert_eq!(
        t.rows,
        [
            pair("Input", "140K"),
            pair("Output", "1.9M"),
            pair("Cache read", "38M"),
            pair("Cache write", "1.0M"),
        ]
    );
    assert_eq!(
        t.models,
        [pair("Opus 5.5", "33M"), pair("Sonnet 5.5", "8.4M")]
    );
    // A day without transcripts keeps its date and usage line, no tokens.
    assert!(vm.bars[0].hover.tokens.is_none());
    assert_eq!(
        vm.bars[0].hover.usage_line.as_deref(),
        Some("0% of the 14.3% daily allowance")
    );
}

/// Cursor and OpenCode: their per-day tally reaches the label, with the
/// monthly wording and a `+` for unpriced usage.
#[test]
fn test_monthly_provider_bar_hover_shows_tokens_and_unpriced_marker() {
    let cal = Calendar::utc();
    let mut tally = DailyTokenUsageTally::new(cal);
    let counts = |input, output, cache_read, cache_write| TokenCounts {
        input,
        output,
        cache_read,
        cache_write,
    };
    let at = Utc.with_ymd_and_hms(2026, 11, 3, 15, 30, 0).unwrap();
    tally.add_at(
        at,
        "gpt-5",
        counts(900_000, 100_000, 2_000_000, 0),
        Some(4.2),
    );
    tally.add_at(at, "Mystery", counts(0, 50_000, 0, 0), None);
    let by_day: std::collections::HashMap<_, _> = tally.into_days().into_iter().collect();
    let days = attaching_tokens_map(week_bars(), &by_day, &cal);

    for provider in [ProviderId::Cursor, ProviderId::OpenCode] {
        let vm = card(&days, provider, AllowanceNoun::Monthly);
        let h = &vm.bars[1].hover;
        assert_eq!(
            h.usage_line.as_deref(),
            Some("14.3% of monthly · 100% of daily allowance")
        );
        let t = h.tokens.as_ref().expect("token block");
        assert_eq!(t.total_text, "3.1M");
        assert_eq!(t.cost_text.as_deref(), Some("≈ $4.20+"));
        assert_eq!(
            t.rows,
            [
                pair("Input", "900K"),
                pair("Output", "150K"),
                pair("Cache read", "2.0M"),
                pair("Cache write", "0"),
            ]
        );
        assert_eq!(t.models[0], pair("gpt-5", "3.0M"));
    }
}

/// A fully unpriced day shows the total but no dollar amount.
#[test]
fn test_unpriced_only_day_shows_no_dollars() {
    let cal = Calendar::utc();
    let mut tally = DailyTokenUsageTally::new(cal);
    let at = Utc.with_ymd_and_hms(2026, 11, 3, 9, 0, 0).unwrap();
    tally.add_at(
        at,
        "Mystery",
        TokenCounts {
            output: 10_000,
            ..TokenCounts::default()
        },
        None,
    );
    let by_day: std::collections::HashMap<_, _> = tally.into_days().into_iter().collect();
    let days = attaching_tokens_map(week_bars(), &by_day, &cal);
    let vm = card(&days, ProviderId::OpenCode, AllowanceNoun::Monthly);
    let t = vm.bars[1].hover.tokens.as_ref().unwrap();
    assert_eq!(t.total_text, "10K");
    assert_eq!(t.cost_text, None);
}
