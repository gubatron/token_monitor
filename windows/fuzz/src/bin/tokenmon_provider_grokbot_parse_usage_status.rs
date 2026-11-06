//! Invariants: `parse_usage_status` never panics on any body (JSON, HTML or
//! binary); errors are only Unauthorized, BadResponse or Custom; on Ok the used
//! percent is in 0..=100 and the derived period length is a positive number
//! of days.
#![no_main]

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ProviderError;
use tokenmon_core::usage::Calendar;
use tokenmon_provider_grokbot::parse_usage_status;

fuzz_target!(|data: &[u8]| {
    let now = Utc
        .timestamp_opt(1_787_000_000, 0)
        .single()
        .expect("valid instant");
    match parse_usage_status(data, Some("user@example.com"), now) {
        Ok(snapshot) => {
            assert!((0.0..=100.0).contains(&snapshot.used_percent));
            assert!((0.0..=100.0).contains(&snapshot.remaining_percent()));
            let cal = Calendar::utc();
            assert!(snapshot.days_in_period(&cal) >= 1);
            let _ = snapshot.usage_pool(&cal);
            let _ = snapshot.entitlement.caption_text();
        }
        Err(
            ProviderError::Unauthorized(_)
            | ProviderError::BadResponse(..)
            | ProviderError::Custom { .. },
        ) => {}
        Err(other) => panic!("unexpected error kind: {other:?}"),
    }
});
