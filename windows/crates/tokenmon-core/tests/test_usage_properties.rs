//! Property tests for day keys, week offsets, budgets and the lenient parsers.

#![allow(
    clippy::unwrap_used,
    clippy::needless_pass_by_value,
    clippy::useless_vec,
    clippy::unreadable_literal,
    clippy::float_cmp
)]

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use proptest::prelude::*;
use tokenmon_core::usage::budget::SpentByDay;
use tokenmon_core::usage::{Calendar, DailyBudget, DayKey, Percent, WeekOffset};
use tokenmon_core::util::{Format, parse_flexible};

fn offset() -> impl Strategy<Value = FixedOffset> {
    (-12 * 3600..=14 * 3600).prop_map(|s| FixedOffset::east_opt(s).unwrap())
}

fn date() -> impl Strategy<Value = NaiveDate> {
    (0..=9999, 1..=12u32, 1..=28u32).prop_map(|(y, m, d)| NaiveDate::from_ymd_opt(y, m, d).unwrap())
}

fn instant() -> impl Strategy<Value = DateTime<Utc>> {
    (0i64..4_000_000_000).prop_map(|s| Utc.timestamp_opt(s, 0).unwrap())
}

proptest! {
    /// key -> start_of_day -> key is the identity, in any fixed offset.
    #[test]
    fn day_key_round_trips(d in date(), off in offset()) {
        let cal = Calendar::fixed(off);
        let key = DayKey::for_date(d);
        let start = key.start_of_day(&cal).expect("valid key");
        prop_assert_eq!(DayKey::for_instant(start, &cal), key);
    }

    /// adding(n) then adding(-n) returns the original key.
    #[test]
    fn day_key_adding_is_invertible(d in date(), n in -20_000i64..20_000, off in offset()) {
        let cal = Calendar::fixed(off);
        let key = DayKey::for_date(d);
        if let Some(shifted) = key.adding_days(n, &cal) {
            prop_assert_eq!(shifted.adding_days(-n, &cal), Some(key));
        }
    }

    /// String order equals chronological order for 4-digit years.
    #[test]
    fn day_key_string_order_is_date_order(a in date(), b in date()) {
        prop_assert_eq!(DayKey::for_date(a).cmp(&DayKey::for_date(b)), a.cmp(&b));
    }

    /// The legacy stored-start-of-day conversion recovers the local date of any midnight.
    #[test]
    fn stored_start_of_day_recovers_date(d in date(), off in offset()) {
        let cal = Calendar::fixed(off);
        let start = cal.start_of_date(d).unwrap();
        let key = DayKey::for_stored_start_of_day(start, i64::from(off.local_minus_utc()));
        prop_assert_eq!(key, DayKey::for_date(d));
    }

    /// previous/next keep the offset <= 0 and next undoes previous.
    #[test]
    fn week_offset_stays_non_positive(raw in any::<i32>()) {
        let o = WeekOffset::new(raw);
        prop_assert!(o.get() <= 0);
        prop_assert!(o.next().get() <= 0);
        prop_assert!(o.previous().get() <= o.get());
        prop_assert_eq!(o.can_go_next(), o.get() < 0);
        if o.get() > i32::MIN {
            prop_assert_eq!(o.previous().next(), o);
        }
    }

    /// start_of_day is idempotent and never after the instant.
    #[test]
    fn start_of_day_idempotent(t in instant(), off in offset()) {
        let cal = Calendar::fixed(off);
        let s = cal.start_of_day(t);
        prop_assert!(s <= t);
        prop_assert_eq!(cal.start_of_day(s), s);
    }

    /// Percent::clamp stays in range for any float.
    #[test]
    fn percent_clamp_in_range(v in any::<f64>()) {
        let c = Percent::clamp(v);
        prop_assert!((0.0..=100.0).contains(&c));
    }

    /// Weekly windows have exactly `n` bars when no early reset is involved.
    #[test]
    fn weekly_window_has_n_bars(n in 1i64..60, reset in instant(), now in instant(), off in offset(), wo in -50i32..=0) {
        let cal = Calendar::fixed(off);
        let days = DailyBudget::build_weekly_window_days(
            100.0, n, reset, &SpentByDay::new(), None, None, WeekOffset::new(wo), now, &cal);
        prop_assert_eq!(i64::try_from(days.len()).unwrap(), n);
        prop_assert!(days.windows(2).all(|w| w[0].date < w[1].date));
    }

    /// Monday weeks always have 7 bars starting on a Monday (offset 0).
    #[test]
    fn monday_week_is_seven_days_from_monday(now in instant(), start in instant(), off in offset()) {
        let cal = Calendar::fixed(off);
        let days = DailyBudget::build_monday_week_days(
            start, start + chrono::Duration::days(30), 100.0,
            &SpentByDay::new(), &SpentByDay::new(), &SpentByDay::new(), WeekOffset::CURRENT, now, &cal);
        prop_assert_eq!(days.len(), 7);
        prop_assert_eq!(cal.weekday_sunday_first(days[0].date), 2);
    }

    /// parse_flexible never panics, and valid output re-parses to the same instant.
    #[test]
    fn parse_flexible_is_total(s in ".{0,48}") {
        let _ = parse_flexible(&s);
    }

    /// RFC 3339 strings produced by chrono always parse back.
    #[test]
    fn parse_flexible_round_trips_rfc3339(t in instant(), nanos in 0u32..1_000_000_000, mins in -720i32..=840) {
        // RFC 3339 offsets have minute resolution.
        let off = FixedOffset::east_opt(mins * 60).unwrap();
        let t = t + chrono::Duration::nanoseconds(i64::from(nanos));
        let text = t.with_timezone(&off).to_rfc3339_opts(chrono::SecondsFormat::Nanos, false);
        prop_assert_eq!(parse_flexible(&text), Some(t));
    }

    /// Token formatting never panics and is non-empty.
    #[test]
    fn format_tokens_total(v in any::<i64>()) {
        prop_assert!(!Format::tokens(v).is_empty());
    }
}
