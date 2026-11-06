//! Shared helpers for the storage tests.
#![allow(dead_code, clippy::unwrap_used)]

use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use tokenmon_core::ManualClock;

/// 2026-10-10 00:00 UTC, the "today" of every storage test.
pub fn today_midnight() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, 0, 0, 0).unwrap()
}

/// An instant `days` calendar days and `hour` hours after today's UTC midnight.
pub fn at(days: i64, hour: i64) -> DateTime<Utc> {
    today_midnight() + Duration::days(days) + Duration::hours(hour)
}

/// A UTC clock positioned at `now`.
pub fn utc_clock(now: DateTime<Utc>) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(now))
}

/// A clock positioned at `now` with a fixed local offset of `hours` east of UTC.
pub fn offset_clock(now: DateTime<Utc>, hours: i32) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(now).with_offset(FixedOffset::east_opt(hours * 3600).unwrap()))
}
