//! Ports of `GrokbotUsagePollerTests.swift` (spec 02 section 5.10) and the
//! history-retention test of `AuthInvalidationTests`.

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use tokenmon_core::storage::{InMemoryStringStore, QuotaWindowTransition, TextStore};
use tokenmon_core::usage::{Calendar, DailyBudget, DailyBudgetDay, UsagePool, WeekOffset};
use tokenmon_core::{Clock, ManualClock, ProviderId};
use tokenmon_provider_grokbot::{GrokbotEntitlement, GrokbotPublisher, GrokbotSnapshot};

fn utc(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
}

fn snap(
    used: f64,
    start: Option<DateTime<Utc>>,
    reset: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
) -> GrokbotSnapshot {
    GrokbotSnapshot {
        fetched_at: at,
        used_percent: used,
        period_start: start,
        resets_at: reset,
        entitlement: GrokbotEntitlement::Cursor,
        account_email: None,
        has_included_allowance: true,
    }
}

fn publisher(now: DateTime<Utc>) -> (GrokbotPublisher, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(now));
    let store: Arc<dyn TextStore> = Arc::new(InMemoryStringStore::default());
    (
        GrokbotPublisher::new(&store, Arc::clone(&clock) as Arc<dyn Clock>),
        clock,
    )
}

fn days_of(p: &GrokbotPublisher) -> Vec<DailyBudgetDay> {
    p.daily_budget_days(WeekOffset::CURRENT)
}

fn cal() -> Calendar<'static> {
    Calendar::utc()
}

/// Reset Aug 27 11:00, now Aug 25 15:00: seven bars Aug 20 to 26, 100/7 each.
#[test]
fn test_bars_anchor_to_the_provider_reset_instant() {
    let now = utc(2026, 8, 25, 15);
    let (mut p, _) = publisher(now);
    p.publish(snap(
        10.0,
        Some(utc(2026, 8, 20, 11)),
        Some(utc(2026, 8, 27, 11)),
        now,
    ));
    let days = days_of(&p);
    assert_eq!(days.len(), 7);
    assert_eq!(days[0].date, utc(2026, 8, 20, 0));
    assert_eq!(days[6].date, utc(2026, 8, 26, 0));
    assert!(
        days.iter()
            .all(|d| (d.budget_usd - 100.0 / 7.0).abs() < 1e-9)
    );
}

/// Without a reset there are no bars.
#[test]
fn test_no_reset_yields_no_bars() {
    let now = utc(2026, 8, 25, 15);
    let (mut p, _) = publisher(now);
    p.publish(snap(10.0, None, None, now));
    assert!(days_of(&p).is_empty());
}

/// On reset morning (before the reset instant) the first bar is still the old
/// period start.
#[test]
fn test_reset_morning_keeps_period_start_as_first_bar() {
    let now = utc(2026, 8, 27, 9);
    let (mut p, _) = publisher(now);
    p.publish(snap(
        10.0,
        Some(utc(2026, 8, 20, 11)),
        Some(utc(2026, 8, 27, 11)),
        now,
    ));
    let days = days_of(&p);
    assert_eq!(days.len(), 7);
    assert_eq!(days[0].date, utc(2026, 8, 20, 0));
    assert_eq!(days[6].date, utc(2026, 8, 26, 0));
}

fn span(start_h: Option<(u32, u32)>, end: Option<(u32, u32)>) -> GrokbotSnapshot {
    let at = utc(2026, 8, 25, 12);
    let day = |(d, h): (u32, u32)| utc(2026, 8, d, h);
    snap(1.0, start_h.map(day), end.map(day), at)
}

/// A 14-day span gives 7 days and 7 bars with 100/7 budget.
#[test]
fn test_period_length_is_derived_from_the_snapshot_span() {
    let s = span(Some((13, 11)), Some((27, 11)));
    assert_eq!(s.days_in_period(&cal()), 7);
    let (mut p, _) = publisher(utc(2026, 8, 25, 15));
    p.publish(snap(1.0, s.period_start, s.resets_at, utc(2026, 8, 25, 15)));
    let days = days_of(&p);
    assert_eq!(days.len(), 7);
    assert!((days[0].budget_usd - 100.0 / 7.0).abs() < 1e-9);
}

/// A five day span still yields seven bars.
#[test]
fn test_five_day_span_still_yields_seven_bars() {
    let s = span(Some((22, 11)), Some((27, 11)));
    assert_eq!(s.days_in_period(&cal()), 7);
    let (mut p, _) = publisher(utc(2026, 8, 25, 15));
    p.publish(snap(1.0, s.period_start, s.resets_at, utc(2026, 8, 25, 15)));
    assert_eq!(days_of(&p).len(), 7);
}

/// Calendar days, not truncated hours: Aug 20 12:00 to Aug 27 11:00 is 7.
#[test]
fn test_days_in_period_uses_calendar_days_not_truncated_hours() {
    let s = span(Some((20, 12)), Some((27, 11)));
    assert_eq!(s.days_in_period(&cal()), 7);
    assert_eq!(s.reported_span_days(&cal()), Some(7));
}

/// Same-day start and reset falls back to a week.
#[test]
fn test_same_day_start_and_reset_falls_back_to_a_week() {
    // Swift's inputs: start 01:00 and reset 13:00 on Sep 2, now 14:00 (after the reset).
    let now = utc(2026, 9, 2, 14);
    let start = Some(utc(2026, 9, 2, 1));
    let reset = Some(utc(2026, 9, 2, 13));
    let snapshot = snap(96.0, start, reset, now);
    assert_eq!(snapshot.days_in_period(&cal()), 7);
    let (mut p, _) = publisher(now);
    p.publish(snapshot);
    let days = days_of(&p);
    assert_eq!(days.len(), 7);
    assert!((days[0].budget_usd - 100.0 / 7.0).abs() < 1e-9);
}

/// No period start falls back to seven.
#[test]
fn test_days_in_period_falls_back_to_seven_without_period_start() {
    let s = span(None, Some((27, 11)));
    assert_eq!(s.days_in_period(&cal()), 7);
}

/// The reported span is not sanitised; `days_in_period` is.
#[test]
fn test_reported_span_is_not_sanitized() {
    let s = span(Some((13, 11)), Some((27, 11)));
    assert_eq!(s.reported_span_days(&cal()), Some(14));
    assert_eq!(s.days_in_period(&cal()), 7);
    assert_eq!(span(None, None).reported_span_days(&cal()), None);
    assert_eq!(
        span(Some((27, 11)), Some((20, 11))).reported_span_days(&cal()),
        None
    );
}

/// Pool cadence follows the raw span (spec 02 section 0).
#[test]
fn test_usage_pool_follows_span() {
    let pool = |s: GrokbotSnapshot| s.usage_pool(&cal());
    assert_eq!(
        pool(span(Some((20, 11)), Some((27, 11)))),
        UsagePool::Weekly
    );
    assert_eq!(pool(span(None, None)), UsagePool::Weekly);
    assert_eq!(
        pool(span(Some((1, 11)), Some((31, 11)))),
        UsagePool::Monthly
    );
    assert_eq!(pool(span(Some((10, 11)), Some((27, 11)))), UsagePool::Mixed);
}

/// Entitlement captions.
#[test]
fn test_entitlement_captions() {
    assert_eq!(GrokbotEntitlement::Cursor.caption_text(), "via Cursor");
    assert_eq!(
        GrokbotEntitlement::SuperGrok("SuperGrok Heavy".into()).caption_text(),
        "via SuperGrok Heavy"
    );
    assert_eq!(
        GrokbotEntitlement::SuperGrok(String::new()).caption_text(),
        "via SuperGrok"
    );
}

/// Grokbot is a registered usage provider, polled by `grokbot` and `overview`
/// but not by `cursor`, and it signs in through Cursor.
#[test]
fn test_grokbot_is_a_registered_usage_provider() {
    assert!(ProviderId::USAGE_PROVIDERS.contains(&ProviderId::Grokbot));
    assert_eq!(ProviderId::Grokbot.display_name(), "Grokbot");
    // `selected_tab.polls(provider)`.
    assert!(ProviderId::Grokbot.polls(ProviderId::Grokbot));
    assert!(ProviderId::Overview.polls(ProviderId::Grokbot));
    assert!(!ProviderId::Cursor.polls(ProviderId::Grokbot));
    assert_eq!(ProviderId::Grokbot.sign_in_provider(), ProviderId::Cursor);
}

/// `isRollover`: +7 days true, +1 day false, nil previous false, backwards false.
#[test]
fn test_is_new_window_detects_rollover() {
    let a = utc(2026, 8, 20, 11);
    let r = |p, n| QuotaWindowTransition::is_rollover(p, n, 7);
    assert!(r(Some(a), Some(a + Duration::days(7))));
    assert!(!r(Some(a), Some(a + Duration::days(1))));
    assert!(!r(None, Some(a)));
    assert!(!r(Some(a), Some(a - Duration::days(7))));
}

/// Polls 10, 30, 50 then an early reset to 3: seven window bars from today and
/// three prior bars, with pace restarting today.
#[test]
fn test_early_reset_keeps_history_and_anchors_bars_to_new_period_start() {
    let t0 = utc(2026, 8, 25, 12);
    let (mut p, clock) = publisher(t0 - Duration::days(3));
    let (start1, reset1) = (t0 - Duration::days(3), t0 + Duration::days(4));
    for (used, back) in [(10.0, 3), (30.0, 2), (50.0, 1)] {
        let at = t0 - Duration::days(back);
        clock.set(at);
        p.publish(snap(used, Some(start1), Some(reset1), at));
    }
    clock.set(t0);
    let transition = p.publish(snap(3.0, Some(t0), Some(t0 + Duration::days(7)), t0));
    assert_eq!(transition, QuotaWindowTransition::EarlyReset);

    let days = days_of(&p);
    let (prior, window): (Vec<_>, Vec<_>) = days.iter().partition(|d| d.is_prior_window);
    assert_eq!(window.len(), 7);
    assert_eq!(window[0].date, utc(2026, 8, 25, 0));
    assert!((window[0].spent_usd - 3.0).abs() < 1e-9);
    assert_eq!(prior.len(), 3);
    assert_eq!(prior[0].date, utc(2026, 8, 22, 0));
    assert!((prior[1].spent_usd - 20.0).abs() < 1e-9);
    assert!((prior[2].spent_usd - 20.0).abs() < 1e-9);

    let c = cal();
    assert_eq!(
        DailyBudget::weekly_pace_period_start(&days, &c),
        Some(utc(2026, 8, 25, 0))
    );
    let pace =
        DailyBudget::pace_headroom(&days, 3.0, None, Some(t0 + Duration::days(7)), t0, &c).unwrap();
    assert!((pace.earned - 100.0 / 7.0).abs() < 1e-9);
    assert!((pace.headroom_today - (100.0 / 7.0 - 3.0)).abs() < 1e-9);
}

/// The new period start wins when the reset instant did not change.
#[test]
fn test_new_period_start_wins_over_unchanged_reset_instant() {
    let t0 = utc(2026, 8, 25, 12);
    let reset = t0 + Duration::days(7);
    let old_start = t0 - Duration::days(3);
    let (mut p, clock) = publisher(old_start);
    p.publish(snap(60.0, Some(old_start), Some(reset), old_start));
    clock.set(t0);
    let transition = p.publish(snap(10.0, Some(t0), Some(reset), t0));
    assert_eq!(transition, QuotaWindowTransition::EarlyReset);
    assert_eq!(p.daily().interrupted_window_start(), Some(old_start));
    assert_eq!(p.daily().window_start(), Some(t0));
    let window: Vec<_> = days_of(&p)
        .into_iter()
        .filter(|d| !d.is_prior_window)
        .collect();
    assert_eq!(window.len(), 7);
    assert_eq!(window[0].date, utc(2026, 8, 25, 0));
}

/// A natural rollover shows no prior-window bars, and the store keeps yesterday.
#[test]
fn test_normal_rollover_shows_no_prior_window_bars() {
    let t0 = utc(2026, 8, 25, 12);
    let reset1 = t0 - Duration::hours(2);
    let (mut p, clock) = publisher(t0 - Duration::days(2));
    for (used, back) in [(0.0, 2), (40.0, 1)] {
        let at = t0 - Duration::days(back);
        clock.set(at);
        p.publish(snap(
            used,
            Some(reset1 - Duration::days(7)),
            Some(reset1),
            at,
        ));
    }
    clock.set(t0);
    let transition = p.publish(snap(
        3.0,
        Some(reset1),
        Some(reset1 + Duration::days(7)),
        t0,
    ));
    assert_eq!(transition, QuotaWindowTransition::Rollover);
    let days = days_of(&p);
    assert_eq!(days.len(), 7);
    assert!(days.iter().all(|d| !d.is_prior_window));
    assert_eq!(days[0].date, utc(2026, 8, 25, 0));
    assert!((days[0].spent_usd - 3.0).abs() < 1e-9);
    assert!((p.daily().day_totals()["2026-08-24"] - 40.0).abs() < 1e-9);
}

/// A 60 to 20 drop inside the same period is not an early reset.
#[test]
fn test_same_period_drop_is_not_treated_as_early_reset() {
    let t0 = utc(2026, 8, 25, 12);
    let (start, reset) = (t0 - Duration::days(1), t0 + Duration::days(6));
    let (mut p, clock) = publisher(t0 - Duration::hours(2));
    p.publish(snap(
        60.0,
        Some(start),
        Some(reset),
        t0 - Duration::hours(2),
    ));
    clock.set(t0);
    let transition = p.publish(snap(20.0, Some(start), Some(reset), t0));
    assert_eq!(transition, QuotaWindowTransition::None);
    assert_eq!(p.daily().interrupted_window_start(), None);
    assert!(days_of(&p).iter().all(|d| !d.is_prior_window));
}

/// Sign-out drops the snapshot but keeps history; account reset clears it.
#[test]
fn test_keeps_history_while_signed_out() {
    let t0 = utc(2026, 8, 25, 12);
    let reset = t0 + Duration::days(2);
    let (mut p, clock) = publisher(t0 - Duration::days(1));
    p.publish(snap(
        10.0,
        Some(t0 - Duration::days(5)),
        Some(reset),
        t0 - Duration::days(1),
    ));
    clock.set(t0);
    p.publish(snap(30.0, Some(t0 - Duration::days(5)), Some(reset), t0));
    let before = p.daily().day_totals().clone();
    assert!(!before.is_empty());

    p.clear_snapshot();
    assert!(p.snapshot().is_none());
    assert!(
        days_of(&p).is_empty(),
        "no reset is remembered after sign-out"
    );
    assert_eq!(p.daily().day_totals(), &before);

    p.account_reset();
    assert!(p.daily().day_totals().is_empty());
}

/// Bars from a remembered reset survive until sign-out clears it.
#[test]
fn test_bars_use_remembered_reset_when_snapshot_lacks_one() {
    let t0 = utc(2026, 8, 25, 12);
    let reset = utc(2026, 8, 27, 11);
    let (mut p, _) = publisher(t0);
    p.publish(snap(10.0, Some(utc(2026, 8, 20, 11)), Some(reset), t0));
    p.publish(snap(12.0, None, None, t0));
    assert_eq!(days_of(&p).len(), 7);
}
