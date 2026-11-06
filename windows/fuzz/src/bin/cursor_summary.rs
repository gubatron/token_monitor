//! Invariants: `parse_summary` and `parse_email` never panic on any bytes; an
//! Ok snapshot has every percent in 0..=100, Total first, finite money; the
//! email is only ever a string containing `@`.
#![no_main]

use chrono::DateTime;
use libfuzzer_sys::fuzz_target;
use tokenmon_provider_cursor::{parse_email, parse_summary};

fuzz_target!(|data: &[u8]| {
    let now = DateTime::from_timestamp(1_786_000_000, 0).expect("fixed instant");
    if let Ok(snap) = parse_summary(data, now) {
        assert!((0.0..=100.0).contains(&snap.used_percent));
        assert_eq!(snap.pools[0].used_percent, snap.used_percent);
        for pool in &snap.pools {
            assert!((0.0..=100.0).contains(&pool.used_percent));
            assert!((0.0..=100.0).contains(&pool.remaining_percent()));
        }
        for usd in [
            snap.plan_used_usd,
            snap.plan_limit_usd,
            snap.on_demand_used_usd,
            snap.on_demand_limit_usd,
        ]
        .into_iter()
        .flatten()
        {
            assert!(usd.is_finite());
        }
        let _ = snap.display_plan_name();
    }
    if let Some(email) = parse_email(data) {
        assert!(email.contains('@'));
    }
});
