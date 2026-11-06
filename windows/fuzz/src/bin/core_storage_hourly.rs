//! Fuzzes the hourly activity payload decoder and the store loading it.
//!
//! Invariants: decoding never panics; a decoded payload re-encodes to JSON that
//! decodes to an equal payload; a store built over arbitrary bytes accepts
//! samples and leaves a valid payload behind.
#![no_main]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ManualClock;
use tokenmon_core::storage::{
    HourlyDeltaActivityStore, InMemoryStringStore, TextStore, decode_hourly_payload,
};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(payload) = decode_hourly_payload(text) {
        let json = serde_json::to_string(&payload).expect("a decoded payload encodes");
        assert_eq!(decode_hourly_payload(&json), Some(payload));
    }
    let now = Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap();
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(text, "k");
    let mut store =
        HourlyDeltaActivityStore::new(backing.clone(), "k", Arc::new(ManualClock::new(now)));
    store.record(10.0, now);
    store.record(20.0, now);
    assert!(decode_hourly_payload(&backing.value("k").expect("persisted")).is_some());
    assert!(store.hour_weights().iter().all(|w| w.is_finite()));
});
