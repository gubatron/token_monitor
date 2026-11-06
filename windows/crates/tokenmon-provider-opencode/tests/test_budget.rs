//! Ports of the month-budget tests of `OpenCodeStatsTests` and
//! `OpenCodeMonthBudgetWeekTests` (spec 03 sections 3.8 and 7.7), plus
//! `testPreviewSnapshot`.
#![allow(clippy::unwrap_used, clippy::unreadable_literal)]

mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use common::{Fixture, Msg};
use tokenmon_core::usage::budget::SpentByDay;
use tokenmon_core::usage::{Calendar, DailyBudgetDay, UsagePool, WeekOffset};
use tokenmon_provider_opencode::budget::{
    MonthBudgetRequest, OpenCodeMonthBudget, ScanFn, daily_tokens, month_daily_budget_days,
};
use tokenmon_provider_opencode::db::{ScanCache, load_scan};
use tokenmon_provider_opencode::snapshot::{OpenCodeSnapshot, WindowKind, WindowUsage};

fn day(m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, m, d, h, 0, 0).unwrap()
}

fn scan_fn(f: &Fixture) -> Arc<ScanFn> {
    let path = f.path.clone();
    let cache = ScanCache::default();
    Arc::new(move |since| load_scan(&path, since, &cache))
}

fn request(
    now: DateTime<Utc>,
    used: f64,
    resets: Option<DateTime<Utc>>,
) -> MonthBudgetRequest<'static> {
    MonthBudgetRequest {
        limit_usd: 60.0,
        used_percent: used,
        period_resets_at: resets,
        week_offset: WeekOffset::CURRENT,
        now,
        spent_by_day: None,
    }
}

fn spent_on(days: &[DailyBudgetDay], date: DateTime<Utc>) -> f64 {
    days.iter()
        .find(|d| d.date == date)
        .map_or(-1.0, |d| d.spent_usd)
}

fn sum(days: &[DailyBudgetDay]) -> f64 {
    days.iter().map(|d| d.spent_usd).sum()
}

fn go_anchor(f: &mut Fixture, secs: i64) {
    f.session("anchor", secs, "opencode-go", "m", false);
}

/// Spends anchor to the Monthly bar's used percent, keeping their ratio.
#[test]
fn test_month_daily_budget_days_anchors_bars_to_used_percent() {
    let mut f = Fixture::new();
    go_anchor(&mut f, day(8, 1, 0).timestamp());
    let spends: SpentByDay = [(day(8, 4, 0), 3.0), (day(8, 5, 0), 7.0)].into();
    let req = MonthBudgetRequest {
        spent_by_day: Some(&spends),
        ..request(day(8, 5, 12), 30.0, None)
    };
    let budget = month_daily_budget_days(&req, scan_fn(&f).as_ref(), &Calendar::utc()).unwrap();
    assert_eq!(budget.days.len(), 7);
    assert_eq!(budget.period_start, day(8, 1, 0));
    assert!((sum(&budget.days) - 30.0).abs() < 0.001);
    assert!((spent_on(&budget.days, day(8, 4, 0)) - 9.0).abs() < 0.001);
    assert!((spent_on(&budget.days, day(8, 5, 0)) - 21.0).abs() < 0.001);
}

/// The console reset (Aug 20) wins over the Aug 1 local anniversary.
#[test]
fn test_month_daily_budget_days_prefers_console_resets_at_over_local_anniversary() {
    let mut f = Fixture::new();
    go_anchor(&mut f, day(8, 1, 0).timestamp());
    let spends: SpentByDay = [(day(8, 4, 0), 3.0), (day(8, 5, 0), 7.0)].into();
    let req = MonthBudgetRequest {
        spent_by_day: Some(&spends),
        ..request(day(8, 5, 12), 30.0, Some(day(8, 20, 0)))
    };
    let budget = month_daily_budget_days(&req, scan_fn(&f).as_ref(), &Calendar::utc()).unwrap();
    assert_eq!(budget.period_start, day(7, 20, 0));
}

/// No console reset and no Go session gives no bars.
#[test]
fn test_month_daily_budget_days_nil_without_signals() {
    let f = Fixture::new();
    let spends = SpentByDay::new();
    let req = MonthBudgetRequest {
        spent_by_day: Some(&spends),
        ..request(day(8, 5, 12), 30.0, None)
    };
    assert!(month_daily_budget_days(&req, scan_fn(&f).as_ref(), &Calendar::utc()).is_none());
}

/// A database that cannot be read behaves like no Go session.
#[test]
fn test_month_daily_budget_days_nil_when_database_is_missing() {
    let load: Box<ScanFn> =
        Box::new(|_| Err(tokenmon_provider_opencode::db::DbError::Missing("x".into())));
    let req = request(day(8, 5, 12), 30.0, None);
    assert!(month_daily_budget_days(&req, load.as_ref(), &Calendar::utc()).is_none());
}

/// Reset Sep 5, now Aug 20: the period has 31 days, so a bar's budget is 100/31.
#[test]
fn test_monthly_daily_budget_uses_subscription_month_denominator() {
    let f = Fixture::new();
    let spends = SpentByDay::new();
    let req = MonthBudgetRequest {
        spent_by_day: Some(&spends),
        ..request(day(8, 20, 12), 20.0, Some(day(9, 5, 0)))
    };
    let budget = month_daily_budget_days(&req, scan_fn(&f).as_ref(), &Calendar::utc()).unwrap();
    let first = budget.days.first().unwrap().budget_usd;
    assert!((first - 100.0 / 31.0).abs() < 0.01);
    assert!(first < 5.0);
}

/// With no local spends the used percent is spread over the elapsed days.
#[test]
fn test_monthly_daily_budget_spreads_when_no_local_spends() {
    let f = Fixture::new();
    let spends = SpentByDay::new();
    let now = day(8, 7, 12);
    let req = MonthBudgetRequest {
        spent_by_day: Some(&spends),
        ..request(now, 12.0, Some(day(9, 5, 0)))
    };
    let budget = month_daily_budget_days(&req, scan_fn(&f).as_ref(), &Calendar::utc()).unwrap();
    assert!((sum(&budget.days) - 12.0).abs() < 0.01);
    assert!(spent_on(&budget.days, day(8, 7, 0)) > 0.0);
}

fn week_offset_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.insert(
        "s",
        Msg::go("s", day(7, 20, 12).timestamp(), "m", 6.0),
        false,
    );
    f.insert(
        "s2",
        Msg::go("s2", day(8, 18, 12).timestamp(), "m", 4.0),
        false,
    );
    f
}

/// History before the console window stays out of the headline scale, and
/// the week arrow still paints it from local history.
#[test]
fn test_month_daily_budget_week_offset_keeps_history_out_of_headline_scale() {
    let f = week_offset_fixture();
    let load = scan_fn(&f);
    let cal = Calendar::utc();
    let now = day(8, 20, 12);
    let resets = Some(day(9, 5, 0));

    let current =
        month_daily_budget_days(&request(now, 20.0, resets), load.as_ref(), &cal).unwrap();
    assert!((sum(&current.days) - 20.0).abs() < 0.01);
    assert!((spent_on(&current.days, day(8, 18, 0)) - 20.0).abs() < 0.01);

    let previous_req = MonthBudgetRequest {
        week_offset: WeekOffset::new(-4),
        ..request(now, 20.0, resets)
    };
    let previous = month_daily_budget_days(&previous_req, load.as_ref(), &cal).unwrap();
    assert_eq!(previous.days[0].date, day(7, 20, 0));
    assert!((previous.days[0].spent_usd - 10.0).abs() < 0.01);
    assert!(!previous.days[0].is_prior_window);
}

/// Browsing from the stored percents matches a fresh build at that offset.
#[test]
fn test_days_at_matches_a_fresh_build_at_the_same_offset() {
    let f = week_offset_fixture();
    let load = scan_fn(&f);
    let cal = Calendar::utc();
    let now = day(8, 20, 12);
    let resets = Some(day(9, 5, 0));
    let current: OpenCodeMonthBudget =
        month_daily_budget_days(&request(now, 20.0, resets), load.as_ref(), &cal).unwrap();
    let fresh = month_daily_budget_days(
        &MonthBudgetRequest {
            week_offset: WeekOffset::new(-4),
            ..request(now, 20.0, resets)
        },
        load.as_ref(),
        &cal,
    )
    .unwrap();
    assert_eq!(
        current.days_at(WeekOffset::CURRENT, &cal).unwrap(),
        current.days
    );
    assert_eq!(
        current.days_at(WeekOffset::new(-4), &cal).unwrap(),
        fresh.days
    );
}

/// Without a console reset the local subscription month is used, and its
/// Go-only spends come from the scan.
#[test]
fn test_local_month_budget_reads_go_spends_from_the_scan() {
    let mut f = Fixture::new();
    f.insert(
        "a",
        Msg::go("a", day(8, 1, 12).timestamp(), "m", 3.0),
        false,
    );
    f.insert(
        "b",
        Msg::go("b", day(8, 4, 12).timestamp(), "m", 3.0),
        false,
    );
    f.insert(
        "c",
        Msg::go("c", day(8, 5, 12).timestamp(), "m", 3.0).provider("opencode"),
        false,
    );
    let budget = month_daily_budget_days(
        &request(day(8, 5, 12), 10.0, None),
        scan_fn(&f).as_ref(),
        &Calendar::utc(),
    )
    .unwrap();
    assert_eq!(budget.known_start, Some(day(8, 1, 12)));
    assert_eq!(budget.period_start, day(8, 1, 0));
    // Aug 1 is a Saturday: the bars show Monday Aug 3 on. Go spends on Aug 1
    // and Aug 4 split the 10% evenly; the Zen message on Aug 5 is ignored.
    assert!((spent_on(&budget.days, day(8, 4, 0)) - 5.0).abs() < 0.01);
    assert!(spent_on(&budget.days, day(8, 5, 0)).abs() < 0.01);
}

/// Hover tokens: Go rows only, display model names, unpriced rows carry no cost.
#[test]
fn test_daily_tokens_counts_go_rows_by_display_name() {
    let mut f = Fixture::new();
    f.insert(
        "a",
        Msg::go("a", day(8, 4, 12).timestamp(), "kimi-k2.5", 1.5).tokens([10, 20, 30, 40]),
        false,
    );
    f.insert(
        "z",
        Msg::go("z", day(8, 4, 13).timestamp(), "m", 9.0)
            .provider("opencode")
            .tokens([1, 1, 1, 1]),
        false,
    );
    let cal = Calendar::utc();
    let tokens = daily_tokens(scan_fn(&f).as_ref(), day(7, 1, 0), &cal);
    assert_eq!(tokens.len(), 1);
    let usage = tokens.values().next().unwrap();
    assert_eq!(usage.total_tokens(), 100);
    assert_eq!(usage.models.len(), 1);
    assert_eq!(usage.models[0].name, "Kimi K2.5");
}

/// The preview has three windows, the rolling one first, and the monthly
/// bar is 16.9 of 60.
#[test]
fn test_preview_snapshot() {
    let now = day(8, 20, 12);
    let preview = OpenCodeSnapshot::preview(now);
    assert_eq!(preview.windows.len(), 3);
    assert_eq!(preview.windows[0].kind, WindowKind::Rolling5h);
    assert!((preview.monthly_used_percent() - 16.9 / 60.0 * 100.0).abs() < 0.01);
    assert!(preview.windows.iter().all(|w| w.resets_at.unwrap() > now));
}

/// `testOpenCodeDailyTokens`: two identical rows sum tokens (668), cache reads
/// (600) and billable cost (1), under the display model name.
#[test]
fn test_open_code_daily_tokens_sum_rows_cache_and_cost() {
    let mut f = Fixture::new();
    for id in ["a", "b"] {
        f.insert(
            id,
            Msg::go(id, day(11, 4, 12).timestamp(), "minimax-m3", 0.5).tokens([10, 20, 300, 4]),
            false,
        );
    }
    let cal = Calendar::utc();
    let tokens = daily_tokens(scan_fn(&f).as_ref(), day(10, 1, 0), &cal);
    assert_eq!(tokens.len(), 1);
    let usage = tokens.values().next().unwrap();
    assert_eq!(usage.total_tokens(), 668);
    assert_eq!(usage.cache_read_tokens, 600);
    assert_eq!(usage.cost_usd, Some(1.0));
    let names: Vec<&str> = usage.models.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        [tokenmon_provider_opencode::model_name::display_name("minimax-m3")]
    );
}

fn window(kind: WindowKind) -> WindowUsage {
    WindowUsage {
        kind,
        used_usd: 0.0,
        limit_usd: 100.0,
        resets_at: None,
        session_count: 0,
    }
}

fn snapshot_of(kinds: &[WindowKind]) -> OpenCodeSnapshot {
    let mut s = OpenCodeSnapshot::preview(day(8, 20, 12));
    s.windows = kinds.iter().copied().map(window).collect();
    s
}

/// `testOpenCodeMixedPools`: three stacked windows read `Usage Pool Mixed`.
#[test]
fn test_open_code_mixed_pools() {
    let s = snapshot_of(&[WindowKind::Rolling5h, WindowKind::Weekly, WindowKind::Monthly]);
    assert_eq!(s.usage_pool(), UsagePool::Mixed);
    assert_eq!(s.usage_pool().section_title(), "Usage Pool Mixed");
    assert_eq!(snapshot_of(&[]).usage_pool(), UsagePool::Mixed);
}

/// `testOpenCodeSingleWindowKeepsItsCadence`.
#[test]
fn test_open_code_single_window_keeps_its_cadence() {
    assert_eq!(snapshot_of(&[WindowKind::Monthly]).usage_pool(), UsagePool::Monthly);
    assert_eq!(snapshot_of(&[WindowKind::Weekly]).usage_pool(), UsagePool::Weekly);
    assert_eq!(snapshot_of(&[WindowKind::Rolling5h]).usage_pool(), UsagePool::Hourly);
}
