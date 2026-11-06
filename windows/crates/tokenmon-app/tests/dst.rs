//! Regression tests for #56: panel labels, reset captions and the Overview
//! midnight follow the zone's DST rules instead of one fixed offset.
#![allow(clippy::unwrap_used)]

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use tokenmon_app::daily::{ChartContext, daily_input};
use tokenmon_app::overview::next_midnight;
use tokenmon_core::usage::{Calendar, DailyBudgetDay};
use tokenmon_core::{Clock, ProviderId};
use tokenmon_ui::panels::common::{AllowanceNoun, PanelContext};

/// America/New_York in 2026: EDT (-4) until 06:00Z on Nov 1, then EST (-5);
/// EST until 07:00Z on Mar 8, then EDT.
struct NewYork(DateTime<Utc>);

impl Clock for NewYork {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }

    fn local_offset_at(&self, at: DateTime<Utc>) -> FixedOffset {
        let fall = Utc.with_ymd_and_hms(2026, 11, 1, 6, 0, 0).unwrap();
        let spring = Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap();
        let hours = if at >= spring && at < fall { -4 } else { -5 };
        FixedOffset::east_opt(hours * 3600).unwrap()
    }
}

fn utc(m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, m, d, h, 0, 0).unwrap()
}

/// Scenario 1: Claude bars Oct 31 - Nov 6 straddle the Nov 1 change. Their
/// local midnights are at -4 (Oct 31, Nov 1) then -5; each must keep its own
/// date instead of shifting back a day.
#[test]
fn test_bar_dates_across_the_fall_back_change() {
    let now = utc(11, 6, 17);
    let clock = NewYork(now);
    let cal = Calendar::local(&clock);
    let starts = [
        utc(10, 31, 4),
        utc(11, 1, 4),
        utc(11, 2, 5),
        utc(11, 3, 5),
        utc(11, 4, 5),
        utc(11, 5, 5),
        utc(11, 6, 5),
    ];
    let bars: Vec<_> = starts
        .iter()
        .map(|s| DailyBudgetDay::new(*s, 1.0, 100.0 / 7.0))
        .collect();
    let ctx = ChartContext {
        provider: ProviderId::Claude,
        noun: AllowanceNoun::Weekly,
        period_used_percent: Some(10.0),
        period_start: None,
        resets_at: None,
        week_offset: 0,
        now,
        calendar: &cal,
    };
    let dates: Vec<String> = daily_input(&bars, &ctx)
        .unwrap()
        .days
        .iter()
        .map(|d| d.date.to_string())
        .collect();
    assert_eq!(
        dates,
        [
            "2026-10-31",
            "2026-11-01",
            "2026-11-02",
            "2026-11-03",
            "2026-11-04",
            "2026-11-05",
            "2026-11-06"
        ]
    );
}

/// Scenario 2: on Oct 30 (EDT) a reset on Nov 5 15:00Z is 10:00am EST, not
/// the 11:00am a fixed EDT offset gives.
#[test]
fn test_reset_caption_uses_the_offset_at_the_reset() {
    let clock = NewYork(utc(10, 30, 16));
    let ctx = PanelContext {
        now: clock.now(),
        calendar: Calendar::local(&clock),
    };
    assert_eq!(
        ctx.reset_caption(utc(11, 5, 15)),
        "Resets Thu 05 November 10:00am"
    );
    assert_eq!(ctx.today().to_string(), "2026-10-30");
}

/// The Overview rolls over at local midnight on both sides of a change, and
/// a 25 hour day rolls at the right instant.
#[test]
fn test_next_midnight_across_fall_back() {
    let clock = NewYork(utc(10, 31, 23));
    let cal = Calendar::local(&clock);
    // 23:00 EDT on Oct 31: next midnight is 00:00 EDT Nov 1.
    assert_eq!(next_midnight(utc(11, 1, 3), &cal), utc(11, 1, 4));
    // 05:00 EST on Nov 1 (a 25 hour day): next midnight is 00:00 EST Nov 2.
    assert_eq!(next_midnight(utc(11, 1, 10), &cal), utc(11, 2, 5));
    // And across spring forward (23 hour day): Mar 8 EDT midnight is 04:00Z.
    assert_eq!(
        next_midnight(utc(3, 8, 8), &cal),
        Utc.with_ymd_and_hms(2026, 3, 9, 4, 0, 0).unwrap()
    );
}
