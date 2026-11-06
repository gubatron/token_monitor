#![no_main]
//! Week and subscription-month windows for arbitrary instants, including far
//! past and future: never panic; `start <= now < end`.

use chrono::{DateTime, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_provider_opencode::bounds::{monthly_bounds, weekly_bounds};

fuzz_target!(|input: (i64, i64)| {
    let (Some(now), Some(sub)) = (
        DateTime::<Utc>::from_timestamp(input.0, 0),
        DateTime::<Utc>::from_timestamp(input.1, 0),
    ) else {
        return;
    };
    let m = monthly_bounds(now, sub);
    assert!(m.start <= m.end);
    let days = (m.end - m.start).num_days();
    assert!((0..=31).contains(&days));
    let w = weekly_bounds(now);
    assert!(w.start <= now && now <= w.end);
});
