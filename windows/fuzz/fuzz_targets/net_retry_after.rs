//! Retry-After parsing: never panics; any result is finite and in 0..=3600.
#![no_main]

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::net::{MAX_RETRY_AFTER_SECS, parse_retry_after};

fuzz_target!(|data: &[u8]| {
    let now = Utc.with_ymd_and_hms(2015, 10, 21, 7, 27, 0).unwrap();
    let text = String::from_utf8_lossy(data);
    if let Some(secs) = parse_retry_after(&text, now) {
        assert!(secs.is_finite() && (0.0..=MAX_RETRY_AFTER_SECS).contains(&secs));
    }
});
