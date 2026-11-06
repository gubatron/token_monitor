//! Port of Swift `DailyBudgetTests` and `DailyBudgetWeekOffsetTests` (UTC calendar).

#![allow(
    clippy::unwrap_used,
    clippy::needless_pass_by_value,
    clippy::useless_vec,
    clippy::unreadable_literal,
    clippy::float_cmp
)]

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::usage::budget::SpentByDay;
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay, PaceHeadroom, WeekOffset};

const CAL: Calendar<'static> = Calendar::Fixed(chrono::FixedOffset::east_opt(0).unwrap());

fn date(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
}

fn noon(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    date(y, m, d, 12)
}

fn sod(t: DateTime<Utc>) -> DateTime<Utc> {
    CAL.start_of_day(t)
}

fn same_day(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    CAL.date_of(a) == CAL.date_of(b)
}

fn spent(entries: &[(DateTime<Utc>, f64)]) -> SpentByDay {
    entries.iter().map(|(d, v)| (sod(*d), *v)).collect()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

fn bar(d: DateTime<Utc>, spent: f64, budget: f64) -> DailyBudgetDay {
    DailyBudgetDay::new(sod(d), spent, budget)
}

fn weekly_window(
    limit: f64,
    resets_at: DateTime<Utc>,
    spent_by_day: &SpentByDay,
    now: DateTime<Utc>,
) -> Vec<DailyBudgetDay> {
    DailyBudget::build_weekly_window_days(
        limit,
        7,
        resets_at,
        spent_by_day,
        None,
        None,
        WeekOffset::CURRENT,
        now,
        &CAL,
    )
}

fn pace_of(
    days: &[DailyBudgetDay],
    consumed: f64,
    elapsed: Option<i64>,
    resets: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<PaceHeadroom> {
    DailyBudget::pace_headroom(days, consumed, elapsed, resets, now, &CAL)
}

fn days_ending(today: DateTime<Utc>, count: i64, budget: f64) -> Vec<DailyBudgetDay> {
    (0..count)
        .map(|o| bar(CAL.add_days(today, o - (count - 1)).unwrap(), 0.0, budget))
        .collect()
}

// ---- basics ----

/// Even split of a limit.
#[test]
fn test_budget_per_day_divides_evenly() {
    close(DailyBudget::budget_per_day(100.0, 7), 100.0 / 7.0);
}

/// Zero limit or days gives zero.
#[test]
fn test_budget_per_day_zero_guards() {
    assert_eq!(DailyBudget::budget_per_day(0.0, 7), 0.0);
    assert_eq!(DailyBudget::budget_per_day(100.0, 0), 0.0);
}

/// Inclusive start, exclusive end.
#[test]
fn test_days_in_billing_cycle_counts_inclusive_start_exclusive_end() {
    let n = DailyBudget::days_in_billing_cycle(noon(2026, 8, 1), noon(2026, 8, 8), &CAL);
    assert_eq!(n, 7);
}

/// A cycle is never shorter than one day.
#[test]
fn test_days_in_billing_cycle_minimum_one() {
    let same = noon(2026, 8, 1);
    assert_eq!(DailyBudget::days_in_billing_cycle(same, same, &CAL), 1);
}

/// One bar per day with the per-day budget.
#[test]
fn test_build_days_covers_whole_period_with_per_day_budget() {
    let days = DailyBudget::build_days(
        noon(2026, 8, 1),
        noon(2026, 8, 8),
        70.0,
        &spent(&[(noon(2026, 8, 1), 10.0)]),
        None,
        &CAL,
    );
    assert_eq!(days.len(), 7);
    assert_eq!(days[0].spent_usd, 10.0);
    assert_eq!(days[1].spent_usd, 0.0);
    for d in &days {
        close(d.budget_usd, 10.0);
    }
}

/// Over-budget flag; percent clamps at 100.
#[test]
fn test_over_budget_flag() {
    let over = bar(noon(2026, 8, 1), 15.0, 10.0);
    let under = bar(noon(2026, 8, 2), 5.0, 10.0);
    assert!(over.is_over_budget());
    assert!(!under.is_over_budget());
    assert_eq!(over.percent_of_budget(), 100.0);
}

/// A zero budget reads 0% and never over budget.
#[test]
fn test_percent_of_budget_zero_budget_is_zero() {
    let day = bar(noon(2026, 8, 1), 15.0, 0.0);
    assert_eq!(day.percent_of_budget(), 0.0);
    assert!(!day.is_over_budget());
}

// ---- weekly windows ----

/// Window anchors to the reset instant mid-period.
#[test]
fn test_weekly_window_anchors_to_reset_mid_period() {
    let days = weekly_window(
        70.0,
        date(2026, 8, 27, 11),
        &spent(&[(noon(2026, 8, 22), 3.0)]),
        date(2026, 8, 25, 15),
    );
    assert_eq!(days.len(), 7);
    assert!(same_day(days[0].date, noon(2026, 8, 20)));
    assert!(same_day(days[3].date, noon(2026, 8, 23)));
    assert!(same_day(days[6].date, noon(2026, 8, 26)));
    assert_eq!(days[2].spent_usd, 3.0);
    assert_eq!(days[3].spent_usd, 0.0);
    assert_eq!(days[5].spent_usd, 0.0);
    close(days[0].budget_usd, 10.0);
}

/// Before the reset instant the first bar stays the period's opening day.
#[test]
fn test_weekly_window_before_reset_instant_keeps_period_start_as_first_bar() {
    let before = date(2026, 8, 27, 9);
    let days = weekly_window(
        70.0,
        date(2026, 8, 27, 11),
        &spent(&[(before, 12.0)]),
        before,
    );
    assert_eq!(days.len(), 7);
    assert!(same_day(days[0].date, noon(2026, 8, 20)));
    assert_eq!(CAL.weekday_sunday_first(days[0].date), 5);
    assert!(same_day(days[6].date, noon(2026, 8, 26)));
    assert!(!days.iter().any(|d| same_day(d.date, before)));
}

/// At the reset instant the window rolls to the new period.
#[test]
fn test_weekly_window_rolls_forward_at_reset_instant() {
    let at = date(2026, 8, 27, 11);
    let days = weekly_window(70.0, at, &spent(&[(noon(2026, 8, 21), 40.0)]), at);
    assert_eq!(days.len(), 7);
    assert!(same_day(days[0].date, noon(2026, 8, 27)));
    assert!(same_day(days[6].date, noon(2026, 9, 2)));
    assert!(days.iter().all(|d| d.spent_usd == 0.0));
}

/// A reset lagging several periods still yields a window containing today.
#[test]
fn test_weekly_window_advances_weeks_stale_reset() {
    let now = date(2026, 8, 25, 15);
    let days = weekly_window(70.0, date(2026, 8, 4, 11), &SpentByDay::new(), now);
    assert_eq!(days.len(), 7);
    assert!(same_day(days[0].date, now));
    assert!(same_day(days[6].date, noon(2026, 8, 31)));
}

/// An early reset anchors to the new start and leads with prior days.
#[test]
fn test_weekly_window_anchors_to_early_reset_start_and_leads_with_prior_days() {
    let sp = spent(&[
        (noon(2026, 9, 30), 12.0),
        (noon(2026, 10, 1), 18.0),
        (noon(2026, 10, 2), 3.0),
    ]);
    let days = DailyBudget::build_weekly_window_days(
        100.0,
        7,
        date(2026, 10, 9, 18),
        &sp,
        Some(date(2026, 10, 2, 18)),
        Some(date(2026, 9, 30, 11)),
        WeekOffset::CURRENT,
        date(2026, 10, 3, 9),
        &CAL,
    );
    assert_eq!(days.len(), 9);
    let prior: Vec<f64> = days
        .iter()
        .filter(|d| d.is_prior_window)
        .map(|d| d.spent_usd)
        .collect();
    assert_eq!(prior, [12.0, 18.0]);
    let window: Vec<_> = days.iter().filter(|d| !d.is_prior_window).collect();
    assert_eq!(window.len(), 7);
    assert_eq!(window[0].date, sod(noon(2026, 10, 2)));
    assert_eq!(window[0].spent_usd, 3.0);
    assert_eq!(window[6].date, sod(noon(2026, 10, 8)));
    assert_eq!(
        DailyBudget::weekly_pace_period_start(&days, &CAL),
        Some(sod(noon(2026, 10, 2)))
    );
}

/// Without a window start, bars follow `resets_at`.
#[test]
fn test_weekly_window_without_window_start_is_unchanged() {
    let days = weekly_window(
        100.0,
        date(2026, 10, 9, 18),
        &SpentByDay::new(),
        date(2026, 10, 3, 9),
    );
    assert_eq!(days.len(), 7);
    assert!(days.iter().all(|d| !d.is_prior_window));
    assert_eq!(days[0].date, sod(noon(2026, 10, 2)));
}

/// A window start before the implied one, or in the future, never drags bars back.
#[test]
fn test_window_start_never_moves_bars_before_reset_implied_start() {
    let earlier = DailyBudget::build_weekly_window_days(
        100.0,
        7,
        date(2026, 10, 9, 18),
        &SpentByDay::new(),
        Some(noon(2026, 9, 25)),
        Some(noon(2026, 9, 20)),
        WeekOffset::CURRENT,
        date(2026, 10, 3, 9),
        &CAL,
    );
    assert_eq!(earlier[0].date, sod(noon(2026, 10, 2)));
    assert!(earlier.iter().all(|d| !d.is_prior_window));
    let future = DailyBudget::build_weekly_window_days(
        100.0,
        7,
        date(2026, 10, 9, 18),
        &SpentByDay::new(),
        Some(noon(2026, 10, 5)),
        None,
        WeekOffset::CURRENT,
        date(2026, 10, 3, 9),
        &CAL,
    );
    assert_eq!(future[0].date, sod(noon(2026, 10, 2)));
}

// ---- pace ----

fn sample_days(budget: f64) -> Vec<DailyBudgetDay> {
    let today = noon(2026, 8, 25);
    (0..7)
        .map(|o| bar(CAL.add_days(today, o - 2).unwrap(), 99.0, budget))
        .collect()
}

/// Three elapsed days at 3 per day bank 9.
#[test]
fn test_pace_headroom_zero_consumed_banks_prior_days() {
    let p = pace_of(&sample_days(3.0), 0.0, None, None, noon(2026, 8, 25)).unwrap();
    close(p.daily_budget, 3.0);
    close(p.earned, 9.0);
    close(p.period_consumed, 0.0);
    close(p.headroom_today, 9.0);
}

/// Partial consumption reduces headroom.
#[test]
fn test_pace_headroom_partial_consumed_reduces_headroom() {
    let p = pace_of(&sample_days(3.0), 5.0, None, None, noon(2026, 8, 25)).unwrap();
    close(p.earned, 9.0);
    close(p.headroom_today, 4.0);
}

/// Over pace when consumed exceeds earned.
#[test]
fn test_pace_headroom_over_pace_when_consumed_exceeds_earned() {
    let p = pace_of(&sample_days(3.0), 12.0, None, None, noon(2026, 8, 25)).unwrap();
    close(p.headroom_today, -3.0);
}

/// Today counts, future days do not.
#[test]
fn test_pace_headroom_includes_today_excludes_future_days_from_earned() {
    let days: Vec<_> = (0..7)
        .map(|o| bar(noon(2026, 8, 20 + o), 0.0, 10.0))
        .collect();
    let p = pace_of(&days, 0.0, None, None, noon(2026, 8, 22)).unwrap();
    close(p.earned, 30.0);
    close(p.headroom_today, 30.0);
}

/// No budget gives no pace.
#[test]
fn test_pace_headroom_nil_when_budget_zero() {
    let days = [bar(noon(2026, 8, 25), 0.0, 0.0)];
    assert!(pace_of(&days, 10.0, None, None, noon(2026, 8, 25)).is_none());
    assert!(pace_of(&[], 10.0, None, None, noon(2026, 8, 25)).is_none());
}

/// Monthly 7-bar slice: elapsed days come from the full period.
#[test]
fn test_pace_headroom_uses_elapsed_days_for_full_period() {
    let today = noon(2026, 8, 25);
    let daily = 100.0 / 31.0;
    let days = days_ending(today, 7, daily);
    let p = pace_of(&days, 20.0, Some(25), None, today).unwrap();
    close(p.earned, daily * 25.0);
    close(p.headroom_today, daily * 25.0 - 20.0);
    close(
        pace_of(&days, 20.0, None, None, today).unwrap().earned,
        daily * 7.0,
    );
}

/// Elapsed days are inclusive of today.
#[test]
fn test_elapsed_days_through_today_is_inclusive() {
    let e = |s, n| DailyBudget::elapsed_days_through_today(s, n, &CAL);
    assert_eq!(e(noon(2026, 8, 1), noon(2026, 8, 25)), 25);
    assert_eq!(e(noon(2026, 8, 25), noon(2026, 8, 25)), 1);
    assert_eq!(e(noon(2026, 8, 26), noon(2026, 8, 25)), 0);
}

/// Today is credited on the period's final day.
#[test]
fn test_pace_headroom_credits_today_on_final_period_day() {
    let today = noon(2026, 9, 16);
    let days = days_ending(today, 7, 100.0 / 7.0);
    let elapsed = DailyBudget::elapsed_days_through_today(days[0].date, today, &CAL);
    assert_eq!(elapsed, 7);
    let p = pace_of(&days, 82.0, Some(elapsed), None, today).unwrap();
    close(p.earned, 100.0);
    close(p.headroom_today, 18.0);
}

/// Known start wins for monthly pace.
#[test]
fn test_pace_period_start_prefers_known_start() {
    let days = [bar(noon(2026, 8, 20), 0.0, 100.0 / 31.0)];
    let s = DailyBudget::monthly_pace_period_start(
        &days,
        Some(noon(2026, 8, 5)),
        Some(noon(2026, 9, 5)),
        noon(2026, 8, 20),
        &CAL,
    )
    .unwrap();
    assert!(same_day(s, noon(2026, 8, 5)));
}

/// A 7-bar slice derives its start from `resets_at` minus the period length.
#[test]
fn test_monthly_pace_period_start_derives_from_resets_at_when_slice_is_shorter_than_period() {
    let today = noon(2026, 8, 25);
    let days = days_ending(today, 7, 100.0 / 31.0);
    let s =
        DailyBudget::monthly_pace_period_start(&days, None, Some(noon(2026, 9, 16)), today, &CAL)
            .unwrap();
    assert!(same_day(s, noon(2026, 8, 16)));
}

/// Weekly pace start is the first bar.
#[test]
fn test_weekly_pace_period_start_uses_first_bar() {
    let days: Vec<_> = (0..7)
        .map(|o| bar(noon(2026, 8, 20 + o), 0.0, 100.0 / 7.0))
        .collect();
    let s = DailyBudget::weekly_pace_period_start(&days, &CAL).unwrap();
    assert!(same_day(s, noon(2026, 8, 20)));
}

/// A stale billing cycle paces against the running month.
#[test]
fn test_monthly_pace_period_start_advances_stale_cycle() {
    let today = noon(2026, 8, 25);
    let days = days_ending(today, 7, 100.0 / 31.0);
    let s = DailyBudget::monthly_pace_period_start(
        &days,
        Some(noon(2026, 6, 16)),
        Some(noon(2026, 7, 16)),
        today,
        &CAL,
    )
    .unwrap();
    assert!(same_day(s, noon(2026, 8, 16)));
}

/// Monthly pace never invents a month without signals.
#[test]
fn test_monthly_pace_period_start_nil_without_subscription_signals() {
    let today = noon(2026, 8, 25);
    let days = days_ending(today, 7, 100.0 / 31.0);
    assert!(DailyBudget::monthly_pace_period_start(&days, None, None, today, &CAL).is_none());
}

/// Subscription month from `resets_at` alone.
#[test]
fn test_subscription_month_from_resets_at_alone() {
    let b = DailyBudget::subscription_month(None, Some(noon(2026, 9, 16)), None, &CAL).unwrap();
    assert!(same_day(b.start, noon(2026, 8, 16)));
    assert!(same_day(b.end, noon(2026, 9, 16)));
}

/// No signals, no month.
#[test]
fn test_subscription_month_nil_without_signals() {
    assert!(DailyBudget::subscription_month(None, None, None, &CAL).is_none());
}

/// The 7-day builder needs a cycle and opens on Monday.
#[test]
fn test_build_subscription_month_last_7_days_requires_cycle() {
    let now = noon(2026, 8, 25);
    let none = DailyBudget::build_subscription_month_last_7_days(
        100.0,
        &SpentByDay::new(),
        None,
        None,
        &SpentByDay::new(),
        WeekOffset::CURRENT,
        now,
        &CAL,
    );
    assert!(none.is_none());
    let (days, start) = DailyBudget::build_subscription_month_last_7_days(
        100.0,
        &SpentByDay::new(),
        Some(noon(2026, 8, 16)),
        Some(noon(2026, 9, 16)),
        &SpentByDay::new(),
        WeekOffset::CURRENT,
        now,
        &CAL,
    )
    .unwrap();
    assert_eq!(days.len(), 7);
    assert!(same_day(start, noon(2026, 8, 16)));
    close(days[0].budget_usd, 100.0 / 31.0);
    assert_eq!(CAL.weekday_sunday_first(days[0].date), 2);
    assert!(same_day(days[0].date, noon(2026, 8, 24)));
    assert_eq!(CAL.weekday_sunday_first(days[6].date), 1);
}

/// The running period owns today on reset morning.
#[test]
fn test_monthly_last7_includes_today_on_reset_morning() {
    let morning = date(2026, 8, 16, 9);
    let (days, _) = DailyBudget::build_subscription_month_last_7_days(
        100.0,
        &spent(&[(morning, 12.0)]),
        Some(noon(2026, 7, 17)),
        Some(date(2026, 8, 16, 12)),
        &SpentByDay::new(),
        WeekOffset::CURRENT,
        morning,
        &CAL,
    )
    .unwrap();
    let last = days.last().unwrap();
    assert!(same_day(last.date, morning));
    assert_eq!(last.spent_usd, 12.0);
}

/// A stale monthly cycle advances to the running one.
#[test]
fn test_subscription_month_advances_stale_cycle() {
    let b = DailyBudget::subscription_month(
        Some(noon(2026, 6, 16)),
        Some(noon(2026, 7, 16)),
        Some(noon(2026, 8, 25)),
        &CAL,
    )
    .unwrap();
    assert!(same_day(b.start, noon(2026, 8, 16)));
    assert!(same_day(b.end, noon(2026, 9, 16)));
}

/// Later cycles count from the anchor so Jan 31 returns to the 31st.
#[test]
fn test_subscription_month_advance_keeps_anchor_day_after_short_month() {
    let from_start = DailyBudget::subscription_month(
        Some(noon(2026, 1, 31)),
        None,
        Some(noon(2026, 3, 10)),
        &CAL,
    )
    .unwrap();
    assert!(same_day(from_start.start, noon(2026, 2, 28)));
    assert!(same_day(from_start.end, noon(2026, 3, 31)));
    let from_reset = DailyBudget::subscription_month(
        None,
        Some(noon(2026, 1, 31)),
        Some(noon(2026, 4, 2)),
        &CAL,
    )
    .unwrap();
    assert!(same_day(from_reset.start, noon(2026, 3, 31)));
    assert!(same_day(from_reset.end, noon(2026, 4, 30)));
}

/// The extra reset-day bar keeps the 31-day daily share.
#[test]
fn test_build_days_keeps_daily_share_on_final_morning() {
    let (s, e) = (noon(2026, 7, 16), noon(2026, 8, 16));
    let none = SpentByDay::new();
    let before = DailyBudget::build_days(s, e, 100.0, &none, Some(date(2026, 8, 15, 9)), &CAL);
    let fin = DailyBudget::build_days(s, e, 100.0, &none, Some(date(2026, 8, 16, 9)), &CAL);
    assert_eq!(before.len(), 31);
    assert_eq!(fin.len(), 32);
    close(before[0].budget_usd, 100.0 / 31.0);
    close(fin[0].budget_usd, 100.0 / 31.0);
}

/// A stale monthly end does not paint a closed suffix.
#[test]
fn test_stale_monthly_end_does_not_paint_closed_suffix() {
    let (days, start) = DailyBudget::build_subscription_month_last_7_days(
        100.0,
        &spent(&[(noon(2026, 7, 10), 50.0)]),
        Some(noon(2026, 6, 16)),
        Some(noon(2026, 7, 16)),
        &SpentByDay::new(),
        WeekOffset::CURRENT,
        noon(2026, 8, 25),
        &CAL,
    )
    .unwrap();
    assert!(days.iter().any(|d| same_day(d.date, noon(2026, 8, 25))));
    assert!(!days.iter().any(|d| d.date < sod(noon(2026, 8, 16))));
    assert_eq!(start, sod(noon(2026, 8, 16)));
}

/// One elapsed day credits one day's share.
#[test]
fn test_pace_headroom_period_start_credits_today() {
    let today = noon(2026, 8, 25);
    let p = pace_of(&[bar(today, 0.0, 10.0)], 5.0, Some(1), None, today).unwrap();
    close(p.earned, 10.0);
    close(p.headroom_today, 5.0);
}

/// Elapsed days are capped at the period length.
#[test]
fn test_pace_headroom_caps_elapsed_days_at_period_length() {
    let today = noon(2026, 8, 25);
    let daily = 100.0 / 31.0;
    let p = pace_of(&[bar(today, 0.0, daily)], 50.0, Some(40), None, today).unwrap();
    close(p.earned, daily * 31.0);
    close(p.headroom_today, daily * 31.0 - 50.0);
}

// ---- captions ----

fn mk(daily: f64, earned: f64, consumed: f64, headroom: f64) -> PaceHeadroom {
    PaceHeadroom {
        daily_budget: daily,
        earned,
        period_consumed: consumed,
        headroom_today: headroom,
        resets_at: None,
    }
}

/// Banked caption with no usage.
#[test]
fn test_pace_caption_no_usage_banked() {
    let c = DailyBudget::pace_caption(&mk(10.0, 20.0, 0.0, 20.0));
    assert_eq!(
        c.as_deref(),
        Some("20.0% usage left today from unused prior days")
    );
}

/// No usage yet.
#[test]
fn test_pace_caption_no_usage() {
    let c = DailyBudget::pace_caption(&mk(10.0, 10.0, 0.0, 10.0));
    assert_eq!(c.as_deref(), Some("No usage yet · 10.0% today"));
}

/// No budget, no caption.
#[test]
fn test_pace_caption_nil_without_budget() {
    assert!(DailyBudget::pace_caption(&mk(0.0, 0.0, 0.0, 0.0)).is_none());
}

/// Over pace.
#[test]
fn test_pace_caption_over_pace() {
    let c = DailyBudget::pace_caption(&mk(10.0, 0.0, 55.0, -5.0));
    assert_eq!(c.as_deref(), Some("Usage 5.0% over today's allowance"));
}

/// Banked surplus.
#[test]
fn test_pace_caption_banked_surplus() {
    let c = DailyBudget::pace_caption(&mk(10.0, 0.0, 30.0, 25.0));
    assert_eq!(
        c.as_deref(),
        Some("25.0% usage left today from unused prior days")
    );
}

/// Plain surplus.
#[test]
fn test_pace_caption_surplus() {
    let c = DailyBudget::pace_caption(&mk(10.0, 0.0, 30.0, 4.0));
    assert_eq!(c.as_deref(), Some("4.0% usage left today"));
}

/// On pace.
#[test]
fn test_pace_caption_on_pace() {
    let c = DailyBudget::pace_caption(&mk(10.0, 0.0, 30.0, 0.0));
    assert_eq!(c.as_deref(), Some("On pace for today"));
}

fn weekly_days_ending(today: DateTime<Utc>) -> Vec<DailyBudgetDay> {
    days_ending(today, 7, 100.0 / 7.0)
}

/// Overrun caption states the overrun alone.
#[test]
fn test_over_pace_caption_omits_used_amount() {
    let now = date(2026, 9, 23, 12);
    let reset = Utc.with_ymd_and_hms(2026, 9, 24, 18, 57, 0).unwrap();
    let p = pace_of(
        &weekly_days_ending(sod(now)),
        50.0,
        Some(3),
        Some(reset),
        now,
    )
    .unwrap();
    assert!(p.headroom_today < 0.0);
    let c = DailyBudget::pace_caption(&p).unwrap();
    assert!(!c.contains("used"), "{c}");
    assert!(c.starts_with("Usage 7.1% over today's allowance"), "{c}");
}

/// First day: allowance is one whole day's share.
#[test]
fn test_first_day_remaining_is_whole_day_share() {
    let now = date(2026, 9, 23, 20);
    let reset = date(2026, 9, 30, 19);
    let p = pace_of(
        &weekly_days_ending(sod(now)),
        7.0,
        Some(1),
        Some(reset),
        now,
    )
    .unwrap();
    close(p.earned, 100.0 / 7.0);
    close(p.headroom_today, 100.0 / 7.0 - 7.0);
    assert_eq!(
        DailyBudget::pace_caption(&p).as_deref(),
        Some("7.3% usage left today")
    );
}

/// Final window day accounts for the time left until reset.
#[test]
fn test_final_window_day_accounts_for_time_until_reset() {
    let now = date(2026, 9, 23, 20);
    let reset = Utc.with_ymd_and_hms(2026, 9, 24, 18, 57, 0).unwrap();
    let p = pace_of(
        &weekly_days_ending(sod(now)),
        91.0,
        Some(7),
        Some(reset),
        now,
    )
    .unwrap();
    assert!(p.headroom_today < 0.0);
    let c = DailyBudget::pace_caption(&p).unwrap();
    assert!(c.ends_with("over today's allowance"), "{c}");
}

/// Without a reset the wording follows the day-based allowance.
#[test]
fn test_caption_without_reset_instant() {
    let wed = date(2026, 9, 23, 12);
    let p = pace_of(&weekly_days_ending(sod(wed)), 91.0, Some(7), None, wed).unwrap();
    assert!(p.resets_at.is_none());
    assert_eq!(
        DailyBudget::pace_caption(&p).as_deref(),
        Some("9.0% usage left today")
    );
}

/// Monthly mid-cycle: no false over pace.
#[test]
fn test_monthly_mid_cycle_no_false_over_pace() {
    let now = noon(2026, 8, 6);
    let reset = date(2026, 9, 1, 0);
    let p = pace_of(
        &days_ending(sod(now), 7, 100.0 / 31.0),
        10.0,
        Some(6),
        Some(reset),
        now,
    )
    .unwrap();
    assert!(p.headroom_today > 0.0);
    assert!(!DailyBudget::pace_caption(&p).unwrap().contains("over pace"));
}

/// Monthly burned early flags over pace.
#[test]
fn test_monthly_burned_early_flags_over_pace() {
    let now = noon(2026, 8, 6);
    let reset = date(2026, 9, 1, 0);
    let p = pace_of(
        &days_ending(sod(now), 7, 100.0 / 31.0),
        95.0,
        Some(6),
        Some(reset),
        now,
    )
    .unwrap();
    assert!(p.headroom_today < 0.0);
    let c = DailyBudget::pace_caption(&p).unwrap();
    assert!(c.ends_with("over today's allowance"), "{c}");
}

/// Prior-window days never feed the new window's pace.
#[test]
fn test_pace_headroom_ignores_prior_window_days() {
    let daily = 100.0 / 7.0;
    let mut days: Vec<_> = [noon(2026, 9, 30), noon(2026, 10, 1)]
        .into_iter()
        .map(|d| DailyBudgetDay {
            is_prior_window: true,
            ..bar(d, 20.0, daily)
        })
        .collect();
    days.extend((0..7).map(|o| bar(noon(2026, 10, 2 + o), 0.0, daily)));
    let p = pace_of(&days, 5.0, None, None, date(2026, 10, 3, 9)).unwrap();
    close(p.earned, daily * 2.0);
    close(p.headroom_today, daily * 2.0 - 5.0);
}

// ---- monday weeks ----

/// Prior-window days before a new cycle show preserved spend, flagged.
#[test]
fn test_monday_week_shows_prior_window_days_before_new_cycle_start() {
    let days = DailyBudget::build_monday_week_days(
        date(2026, 10, 2, 18),
        date(2026, 11, 2, 18),
        100.0,
        &spent(&[(noon(2026, 10, 3), 1.5)]),
        &spent(&[(noon(2026, 9, 30), 4.0), (noon(2026, 10, 1), 6.0)]),
        &SpentByDay::new(),
        WeekOffset::CURRENT,
        date(2026, 10, 3, 9),
        &CAL,
    );
    assert_eq!(days.len(), 7);
    let flags: Vec<bool> = days.iter().map(|d| d.is_prior_window).collect();
    assert_eq!(flags, [false, false, true, true, false, false, false]);
    assert_eq!(days[2].spent_usd, 4.0);
    assert_eq!(days[3].spent_usd, 6.0);
    assert_eq!(days[5].spent_usd, 1.5);
}

// ---- week offsets ----

/// The offset never moves past the current week.
#[test]
fn test_week_offset_never_moves_past_the_current_week() {
    let o = WeekOffset::new;
    assert_eq!(o(0).previous().get(), -1);
    assert_eq!(o(-2).previous().get(), -3);
    assert_eq!(o(0).next().get(), 0);
    assert_eq!(o(-1).next().get(), 0);
    assert_eq!(o(-4).next().get(), -3);
    assert!(!o(0).can_go_next());
    assert!(o(-1).can_go_next());
}

/// Chevron-left shows the previous period's bars.
#[test]
fn test_weekly_window_offset_shows_previous_period() {
    let days = DailyBudget::build_weekly_window_days(
        70.0,
        7,
        date(2026, 8, 27, 11),
        &spent(&[(noon(2026, 8, 14), 6.0)]),
        None,
        None,
        WeekOffset::new(-1),
        date(2026, 8, 25, 15),
        &CAL,
    );
    assert_eq!(days.len(), 7);
    assert!(same_day(days[0].date, noon(2026, 8, 13)));
    assert!(same_day(days[6].date, noon(2026, 8, 19)));
    assert_eq!(days[1].spent_usd, 6.0);
    assert!(days.iter().all(|d| !d.is_prior_window));
}

/// Past weeks skip the early-reset lead-in.
#[test]
fn test_weekly_window_offset_skips_early_reset_lead_in() {
    let sp = spent(&[
        (noon(2026, 9, 30), 12.0),
        (noon(2026, 10, 1), 18.0),
        (noon(2026, 10, 2), 3.0),
    ]);
    let days = DailyBudget::build_weekly_window_days(
        100.0,
        7,
        date(2026, 10, 9, 18),
        &sp,
        Some(date(2026, 10, 2, 18)),
        Some(date(2026, 9, 30, 11)),
        WeekOffset::new(-1),
        date(2026, 10, 3, 9),
        &CAL,
    );
    assert_eq!(days.len(), 7);
    assert!(days.iter().all(|d| !d.is_prior_window));
    assert_eq!(days[0].date, sod(noon(2026, 9, 25)));
    assert_eq!(days[6].date, sod(noon(2026, 10, 1)));
    assert_eq!(days[5].spent_usd, 12.0);
    assert_eq!(days[6].spent_usd, 18.0);
}

/// Previous Monday week inside the period keeps recorded spend.
#[test]
fn test_monday_week_offset_shows_previous_week_inside_period() {
    let days = DailyBudget::build_monday_week_days(
        noon(2026, 8, 1),
        noon(2026, 9, 1),
        100.0,
        &spent(&[(noon(2026, 8, 11), 4.0)]),
        &SpentByDay::new(),
        &SpentByDay::new(),
        WeekOffset::new(-1),
        noon(2026, 8, 20),
        &CAL,
    );
    assert_eq!(days.len(), 7);
    assert_eq!(days[0].date, sod(noon(2026, 8, 10)));
    assert_eq!(days[1].spent_usd, 4.0);
    assert!(!days[1].is_prior_window);
}

/// The current week ignores pre-period history.
#[test]
fn test_monday_week_offset_zero_ignores_history_before_period() {
    let days = DailyBudget::build_monday_week_days(
        date(2026, 10, 2, 18),
        date(2026, 11, 2, 18),
        100.0,
        &SpentByDay::new(),
        &SpentByDay::new(),
        &spent(&[(noon(2026, 9, 30), 9.0)]),
        WeekOffset::CURRENT,
        date(2026, 10, 3, 9),
        &CAL,
    );
    let sep30 = days
        .iter()
        .find(|d| same_day(d.date, noon(2026, 9, 30)))
        .unwrap();
    assert_eq!(sep30.spent_usd, 0.0);
    assert!(!sep30.is_prior_window);
}

/// Earlier weeks use history for pre-period days.
#[test]
fn test_monday_week_offset_uses_history_before_period() {
    let days = DailyBudget::build_monday_week_days(
        date(2026, 10, 2, 18),
        date(2026, 11, 2, 18),
        100.0,
        &SpentByDay::new(),
        &SpentByDay::new(),
        &spent(&[(noon(2026, 9, 22), 8.0)]),
        WeekOffset::new(-1),
        date(2026, 10, 3, 9),
        &CAL,
    );
    assert_eq!(days[0].date, sod(noon(2026, 9, 21)));
    assert_eq!(days[1].spent_usd, 8.0);
    assert!(!days[1].is_prior_window);
}

/// A far-future reset cannot allocate unbounded bars.
#[test]
fn test_build_days_caps_absurd_periods() {
    let days = DailyBudget::build_days(
        noon(2026, 1, 1),
        noon(9000, 1, 1),
        100.0,
        &SpentByDay::new(),
        None,
        &CAL,
    );
    assert!(days.len() <= 3_700);
}
