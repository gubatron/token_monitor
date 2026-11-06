//! Regression tests for #68: which local day the OpenCode hourly stacks
//! belong to, across local midnight and across a DST change.
#![allow(clippy::unwrap_used, clippy::unreadable_literal)]

mod common;

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use common::{Fixture, Msg};
use tokenmon_core::usage::Calendar;
use tokenmon_core::{Clock, ManualClock};
use tokenmon_provider_opencode::db::{ScanCache, load_scan, scan_start};
use tokenmon_provider_opencode::stats::{DayHourlyUsage, build_day_hourly_usage};

/// US Mountain time around the 2026 spring change: UTC-7 before 09:00Z on
/// 2026-03-08, UTC-6 from then on.
struct DenverSpringClock(DateTime<Utc>);

impl Clock for DenverSpringClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }

    fn local_offset_at(&self, at: DateTime<Utc>) -> FixedOffset {
        let change = Utc.with_ymd_and_hms(2026, 3, 8, 9, 0, 0).unwrap();
        let hours = if at < change { -7 } else { -6 };
        FixedOffset::east_opt(hours * 3600).expect("offset")
    }
}

fn fixture_with_messages(secs: &[i64]) -> Fixture {
    let mut f = Fixture::new();
    f.session("s", secs[0], "opencode-go", "minimax-m3", false);
    for s in secs {
        f.message(Msg::go("s", *s, "minimax-m3", 1.0));
    }
    f
}

fn hourly_at(f: &Fixture, now: DateTime<Utc>, cal: &Calendar<'_>) -> DayHourlyUsage {
    let scan = load_scan(&f.path, scan_start(now), &ScanCache::default()).expect("scan");
    build_day_hourly_usage(&scan.rows, now, cal, 8)
}

/// Just after local midnight, stacks built before midnight are stamped with
/// yesterday and report themselves stale; a rebuild after midnight holds none
/// of yesterday's hours.
#[test]
fn test_hourly_usage_straddling_local_midnight_is_attributed_to_its_own_day() {
    let clock = ManualClock::new(Utc::now()).with_offset(FixedOffset::west_opt(5 * 3600).unwrap());
    let cal = Calendar::local(&clock);
    // 22:30 on Aug 1 local, then 00:30 on Aug 2 local.
    let before = Utc.with_ymd_and_hms(2026, 8, 2, 3, 30, 0).unwrap();
    let after = Utc.with_ymd_and_hms(2026, 8, 2, 5, 30, 0).unwrap();
    let f = fixture_with_messages(&[before.timestamp(), after.timestamp()]);

    let evening_now = before + Duration::hours(1);
    let evening = hourly_at(&f, evening_now, &cal);
    assert_eq!(evening.day.to_string(), "2026-08-01");
    assert_eq!(
        evening.day_start,
        Utc.with_ymd_and_hms(2026, 8, 1, 5, 0, 0).unwrap()
    );
    assert!((evening.hours[22].total_usd() - 1.0).abs() < 1e-9);
    assert!(evening.is_for_day_of(evening_now, &cal));
    assert!(!evening.is_for_day_of(after, &cal), "stale after midnight");

    let morning = hourly_at(&f, after, &cal);
    assert_eq!(morning.day.to_string(), "2026-08-02");
    assert_eq!(
        morning.day_start,
        Utc.with_ymd_and_hms(2026, 8, 2, 5, 0, 0).unwrap()
    );
    assert!((morning.hours[0].total_usd() - 1.0).abs() < 1e-9);
    assert!(morning.hours[22].segments.is_empty(), "yesterday dropped");
    assert!(morning.is_for_day_of(after, &cal));
}

/// A DST change inside the day: each row's hour uses the offset in force at
/// that row's own instant, and the 23-hour day starts at the pre-change
/// midnight.
#[test]
fn test_hourly_usage_across_a_dst_change_uses_each_rows_own_offset() {
    let now = Utc.with_ymd_and_hms(2026, 3, 8, 20, 0, 0).unwrap();
    let clock = DenverSpringClock(now);
    let cal = Calendar::local(&clock);
    let at = |h: u32, m: u32| {
        Utc.with_ymd_and_hms(2026, 3, 8, h, m, 0)
            .unwrap()
            .timestamp()
    };
    let f = fixture_with_messages(&[
        at(6, 59), // 23:59 MST on Mar 7: yesterday
        at(7, 0),  // 00:00 MST
        at(8, 30), // 01:30 MST
        at(9, 30), // 03:30 MDT (02:xx never exists)
        at(20, 0), // 14:00 MDT
    ]);

    let day = hourly_at(&f, now, &cal);

    assert_eq!(day.day.to_string(), "2026-03-08");
    assert_eq!(
        day.day_start,
        Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap()
    );
    let hours: Vec<u32> = day
        .hours
        .iter()
        .filter(|h| h.message_count > 0)
        .map(|h| h.hour)
        .collect();
    assert_eq!(hours, vec![0, 1, 3, 14]);
    assert!(day.hours[23].segments.is_empty(), "Mar 7 23:59 excluded");
}
