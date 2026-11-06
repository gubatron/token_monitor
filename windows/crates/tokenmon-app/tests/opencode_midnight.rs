//! Regression for #68: OpenCode hourly stacks built before local midnight
//! must not be recorded as today's hours after it.
#![allow(clippy::unwrap_used)]

use chrono::{FixedOffset, TimeZone, Utc};
use tokenmon_app::sources::{TodayPart, opencode_today_part};
use tokenmon_core::ManualClock;
use tokenmon_core::usage::Calendar;
use tokenmon_provider_opencode::stats::{DayHourlyUsage, HourSegment, HourUsage};

fn stacks(day: &str, start: chrono::DateTime<Utc>) -> DayHourlyUsage {
    let hours = (0..24)
        .map(|hour| HourUsage {
            hour,
            segments: if hour == 22 {
                vec![HourSegment {
                    provider_id: "opencode-go".into(),
                    model_id: "m".into(),
                    cost_usd: 3.0,
                    quota_cost: 3.0,
                    total_tokens: 100,
                    messages: 1,
                }]
            } else {
                Vec::new()
            },
            message_count: u32::from(hour == 22),
        })
        .collect();
    DayHourlyUsage {
        day: day.parse().unwrap(),
        day_start: start,
        hours,
        legend: Vec::new(),
    }
}

/// Stacks for yesterday yield nothing just after midnight (and, built for
/// today, are stamped with their own day).
#[test]
fn test_stale_stacks_after_midnight_are_not_recorded_as_today() {
    let clock = ManualClock::new(Utc::now()).with_offset(FixedOffset::west_opt(5 * 3600).unwrap());
    let cal = Calendar::local(&clock);
    let day_start = Utc.with_ymd_and_hms(2026, 8, 1, 5, 0, 0).unwrap();
    let usage = stacks("2026-08-01", day_start);

    let evening = Utc.with_ymd_and_hms(2026, 8, 2, 3, 30, 0).unwrap();
    let Some(TodayPart::OpenCode(day)) = opencode_today_part(&usage, 60.0, evening, &cal) else {
        panic!("same-day stacks are recorded");
    };
    assert_eq!(day.day.to_string(), "2026-08-01");
    assert!(day.go_weights[22] > 0.0);

    let after_midnight = Utc.with_ymd_and_hms(2026, 8, 2, 5, 30, 0).unwrap();
    assert!(opencode_today_part(&usage, 60.0, after_midnight, &cal).is_none());
}
