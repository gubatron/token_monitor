//! Local-calendar helpers driven by an injected [`Clock`] (day starts, local
//! hours, billing-month arithmetic).
//!
//! Stand-ins for the ws-core-usage `DailyBudget`/`DayKey` helpers that this
//! crate needs for aggregation; they use the clock's local UTC offset so tests
//! pick any zone deterministically.

use chrono::{DateTime, Days, FixedOffset, Months, NaiveDate, TimeZone, Timelike, Utc};
use tokenmon_core::Clock;

/// The local calendar date of `at`.
///
/// # Examples
///
/// ```
/// use chrono::{FixedOffset, NaiveDate, TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_provider_cursor::local_date;
///
/// let clock = ManualClock::from_unix(0).with_offset(FixedOffset::west_opt(8 * 3600).unwrap());
/// let at = Utc.with_ymd_and_hms(2026, 3, 1, 3, 0, 0).unwrap();
/// assert_eq!(local_date(&clock, at), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
/// ```
pub fn local_date(clock: &dyn Clock, at: DateTime<Utc>) -> NaiveDate {
    at.with_timezone(&clock.local_offset_at(at)).date_naive()
}

/// The local hour (0..=23) of `at`.
pub fn local_hour(clock: &dyn Clock, at: DateTime<Utc>) -> usize {
    at.with_timezone(&clock.local_offset_at(at)).hour() as usize
}

/// Local midnight at the start of `date`, as a UTC instant.
pub fn local_midnight(clock: &dyn Clock, date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_time(chrono::NaiveTime::MIN);
    let guess = Utc.from_utc_datetime(&naive);
    let offset: FixedOffset = clock.local_offset_at(guess);
    offset
        .from_local_datetime(&naive)
        .single()
        .map_or(guess, |t| t.with_timezone(&Utc))
}

/// `startOfDay(at)` in the clock's local calendar.
pub fn start_of_day(clock: &dyn Clock, at: DateTime<Utc>) -> DateTime<Utc> {
    local_midnight(clock, local_date(clock, at))
}

/// Local midnight `days` calendar days before the day of `at`.
pub fn start_of_day_minus_days(clock: &dyn Clock, at: DateTime<Utc>, days: u64) -> DateTime<Utc> {
    let date = local_date(clock, at);
    let back = date.checked_sub_days(Days::new(days)).unwrap_or(date);
    local_midnight(clock, back)
}

/// `daysInBillingCycle`: whole local calendar days between the two day
/// starts, at least 1.
pub fn days_in_billing_cycle(clock: &dyn Clock, start: DateTime<Utc>, end: DateTime<Utc>) -> i64 {
    (local_date(clock, end) - local_date(clock, start))
        .num_days()
        .max(1)
}

/// `calendar.date(byAdding: .month, value: months, to: at)`, clamping to the
/// length of the target month. `None` when out of range.
pub fn add_months(clock: &dyn Clock, at: DateTime<Utc>, months: i32) -> Option<DateTime<Utc>> {
    let local = at.with_timezone(&clock.local_offset_at(at));
    let shifted = if months >= 0 {
        local.checked_add_months(Months::new(months.unsigned_abs()))
    } else {
        local.checked_sub_months(Months::new(months.unsigned_abs()))
    }?;
    Some(shifted.with_timezone(&Utc))
}

/// `DailyBudget.subscriptionMonth`: the billing-month bounds anchored to the
/// cycle, inferring a missing side one calendar month away and advancing a
/// stale cycle to the running one (bounded to 520 steps).
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_provider_cursor::subscription_month;
///
/// let clock = ManualClock::from_unix(0);
/// let start = Utc.with_ymd_and_hms(2026, 8, 22, 17, 0, 0).unwrap();
/// let (s, e) = subscription_month(&clock, Some(start), None, None).unwrap();
/// assert_eq!(s, start);
/// assert_eq!(e, Utc.with_ymd_and_hms(2026, 9, 22, 17, 0, 0).unwrap());
/// assert!(subscription_month(&clock, None, None, None).is_none());
/// ```
pub fn subscription_month(
    clock: &dyn Clock,
    known_start: Option<DateTime<Utc>>,
    resets_at: Option<DateTime<Utc>>,
    now: Option<DateTime<Utc>>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let (mut start, mut end, anchor, mut end_offset) = match (known_start, resets_at) {
        (Some(s), Some(r)) if r > s => (s, r, r, 0),
        (Some(s), _) => {
            let e = add_months(clock, s, 1).filter(|e| *e > s)?;
            (s, e, s, 1)
        }
        (None, Some(r)) => {
            let s = add_months(clock, r, -1).filter(|s| r > *s)?;
            (s, r, r, 0)
        }
        (None, None) => return None,
    };
    if let Some(now) = now {
        for _ in 0..520 {
            if end > now {
                break;
            }
            let Some(next) = add_months(clock, anchor, end_offset + 1).filter(|n| *n > end) else {
                break;
            };
            start = end;
            end = next;
            end_offset += 1;
        }
    }
    Some((start, end))
}
