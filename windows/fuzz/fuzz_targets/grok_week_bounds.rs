//! Invariants: window bounds and the daily-bar builder terminate and never
//! panic on extreme reset, now and sample instants and week offsets; for sane
//! dates the current window spans 6 or 7 days.
#![no_main]

use chrono::{DateTime, TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ManualClock;
use tokenmon_provider_grok::calendar::{DayCalendar, LocalCalendar};
use tokenmon_provider_grok::daily::{billing_period_week_bounds, week};
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;

fn instant(bytes: &[u8]) -> DateTime<Utc> {
    let mut raw = [0u8; 8];
    raw[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
    let secs = i64::from_le_bytes(raw);
    Utc.timestamp_opt(secs, 0).single().unwrap_or(DateTime::UNIX_EPOCH)
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 25 {
        return;
    }
    let reset = instant(&data[0..8]);
    let now = instant(&data[8..16]);
    let sample_at = instant(&data[16..24]);
    let offset = i64::from(i8::from_le_bytes([data[24]]));
    let clock = ManualClock::new(now);
    let cal = LocalCalendar::new(&clock);

    if let Some((start, end)) = billing_period_week_bounds(Some(reset), 0, &cal, now) {
        let sane = |t: DateTime<Utc>| (-2_000_000_000..4_000_000_000).contains(&t.timestamp());
        if sane(reset) && sane(now) {
            let span = cal.days_between(start, end);
            assert!((6..=7).contains(&span), "span {span}");
        }
    }
    let samples = [
        WeeklyUsageSnapshot::new(sample_at, f64::from(data[24]) / 2.0).with_resets_at(Some(reset)),
        WeeklyUsageSnapshot::new(now, 50.0).with_resets_at(Some(reset)),
    ];
    let _ = week(&samples, samples.last(), offset, Some(reset), &cal, now);
});
