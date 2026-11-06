//! Port of Swift `DayKeyTests` plus injected-calendar and DST behaviour.

#![allow(
    clippy::unwrap_used,
    clippy::needless_pass_by_value,
    clippy::useless_vec,
    clippy::unreadable_literal,
    clippy::float_cmp
)]

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use tokenmon_core::Clock;
use tokenmon_core::usage::{Calendar, DayKey};

fn off(secs: i32) -> FixedOffset {
    FixedOffset::east_opt(secs).unwrap()
}

/// 2026-09-15T06:00:00Z, Denver local midnight (Swift reference 811_144_800).
fn denver_midnight() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 15, 6, 0, 0).unwrap()
}

/// Key round-trips through start-of-day; garbage gives None.
#[test]
fn test_key_round_trips_through_start_of_day() {
    let denver = Calendar::fixed(off(-6 * 3600));
    let date = denver_midnight() + chrono::Duration::hours(15);
    let key = DayKey::for_instant(date, &denver);
    assert_eq!(key.as_str(), "2026-09-15");
    assert_eq!(key.start_of_day(&denver), Some(denver_midnight()));
    assert!(DayKey::from_raw("garbage").start_of_day(&denver).is_none());
}

/// Adding days crosses month and year boundaries.
#[test]
fn test_adding_days_crosses_months() {
    let utc = Calendar::utc();
    let add = |k: &str, n| {
        DayKey::from_raw(k)
            .adding_days(n, &utc)
            .map(|k| k.to_string())
    };
    assert_eq!(add("2026-10-10", -40).as_deref(), Some("2026-08-31"));
    assert_eq!(add("2026-12-31", 1).as_deref(), Some("2027-01-01"));
    assert_eq!(add("nope", 1), None);
}

/// A Denver local midnight keeps its date whatever zone is "current".
#[test]
fn test_stored_start_of_day_keeps_its_date_across_zones() {
    let instant = denver_midnight();
    // Denver -6, Tokyo +9, London (BST) +1, Auckland (NZST) +12, UTC.
    for secs in [-6 * 3600, 9 * 3600, 3600, 12 * 3600, 0] {
        assert_eq!(
            DayKey::for_stored_start_of_day(instant, i64::from(secs)).as_str(),
            "2026-09-15",
            "offset {secs}"
        );
    }
}

/// Tokyo, Kolkata, Auckland and LA midnights round-trip 2026-07-16.
#[test]
fn test_stored_start_of_day_from_eastern_and_half_hour_zones() {
    for secs in [9 * 3600, 5 * 3600 + 1800, 12 * 3600, -7 * 3600] {
        let cal = Calendar::fixed(off(secs));
        let start = DayKey::from_raw("2026-07-16").start_of_day(&cal).unwrap();
        assert_eq!(
            DayKey::for_stored_start_of_day(start, i64::from(secs)).as_str(),
            "2026-07-16",
            "offset {secs}"
        );
    }
}

/// Out-of-range month or day is rejected rather than rolled over.
#[test]
fn test_malformed_keys_are_rejected() {
    let utc = Calendar::utc();
    for bad in [
        "",
        "2026-13-01",
        "2026-02-30",
        "2026-01",
        "a-b-c",
        "2026-01-01-05",
    ] {
        assert!(DayKey::from_raw(bad).start_of_day(&utc).is_none(), "{bad}");
    }
}

/// Keys sort chronologically as strings.
#[test]
fn test_keys_sort_chronologically() {
    let mut keys = vec![
        DayKey::from_raw("2026-10-01"),
        DayKey::from_raw("2025-12-31"),
        DayKey::from_raw("2026-02-03"),
    ];
    keys.sort();
    let text: Vec<_> = keys.iter().map(DayKey::as_str).collect();
    assert_eq!(text, ["2025-12-31", "2026-02-03", "2026-10-01"]);
}

/// Simplified US Mountain clock: DST from 2026-03-08T09:00Z to 2026-11-01T08:00Z.
struct DenverClock;

impl Clock for DenverClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap()
    }
    fn local_offset_at(&self, at: DateTime<Utc>) -> FixedOffset {
        let start = Utc.with_ymd_and_hms(2026, 3, 8, 9, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 11, 1, 8, 0, 0).unwrap();
        if at >= start && at < end {
            off(-6 * 3600)
        } else {
            off(-7 * 3600)
        }
    }
}

/// A clock-backed calendar honours DST: the spring-forward day is 23 hours long.
#[test]
fn test_clock_backed_calendar_handles_dst_day_lengths() {
    let clock = DenverClock;
    let cal = Calendar::local(&clock);
    let mar8 = DayKey::from_raw("2026-03-08").start_of_day(&cal).unwrap();
    assert_eq!(mar8, Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap());
    let mar9 = cal.add_days(mar8, 1).unwrap();
    assert_eq!(mar9, Utc.with_ymd_and_hms(2026, 3, 9, 6, 0, 0).unwrap());
    assert_eq!(cal.day_diff(mar8, mar9), 1);
    assert_eq!(
        DayKey::from_raw("2026-03-08")
            .adding_days(1, &cal)
            .unwrap()
            .as_str(),
        "2026-03-09"
    );
}

/// Shifting outside years 0..=9999 gives None instead of a key that cannot re-parse
/// (regression: `day_key_adding_is_invertible`).
#[test]
fn test_adding_days_outside_four_digit_years_is_none() {
    let utc = Calendar::utc();
    assert!(
        DayKey::from_raw("0000-01-01")
            .adding_days(-1, &utc)
            .is_none()
    );
    assert!(
        DayKey::from_raw("9999-12-31")
            .adding_days(1, &utc)
            .is_none()
    );
    assert!(
        DayKey::from_raw("9999-12-31")
            .adding_days(i64::MAX, &utc)
            .is_none()
    );
}

/// A clock whose offset changes at runtime.
struct MovableClock(std::sync::atomic::AtomicI32);

impl Clock for MovableClock {
    fn now(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }
    fn local_offset_at(&self, _at: DateTime<Utc>) -> FixedOffset {
        off(self.0.load(std::sync::atomic::Ordering::SeqCst))
    }
}

/// A time-zone change takes effect on the next call: the offset is never cached.
#[test]
fn test_calendar_never_caches_the_offset() {
    let clock = MovableClock(std::sync::atomic::AtomicI32::new(0));
    let cal = Calendar::local(&clock);
    let t = Utc.with_ymd_and_hms(2026, 6, 1, 23, 0, 0).unwrap();
    assert_eq!(DayKey::for_instant(t, &cal).as_str(), "2026-06-01");
    clock.0.store(2 * 3600, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(DayKey::for_instant(t, &cal).as_str(), "2026-06-02");
}
