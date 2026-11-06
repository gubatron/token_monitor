//! Fuzzes the daily quota payload decoder and the store loading it.
//!
//! Invariants: decoding never panics; a decoded payload re-encodes to JSON that
//! decodes to an equal payload; a store loaded from arbitrary bytes keeps only
//! valid day keys and rewrites a decodable payload.
#![no_main]

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ManualClock;
use tokenmon_core::storage::{
    DailyQuotaDeltaStore, InMemoryStringStore, TextStore, decode_daily_payload, start_of_day,
};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(payload) = decode_daily_payload(text) {
        let json = serde_json::to_string(&payload).expect("a decoded payload encodes");
        assert_eq!(decode_daily_payload(&json), Some(payload));
    }
    let now = Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap();
    let clock = Arc::new(ManualClock::new(now));
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(text, "k");
    let mut store = DailyQuotaDeltaStore::new(backing.clone(), "k", clock.clone());
    assert!(
        store
            .day_totals()
            .keys()
            .all(|k| start_of_day(clock.as_ref(), k).is_some())
    );
    store.record(10.0, now, None, 7);
    store.record(20.0, now, None, 7);
    assert!(decode_daily_payload(&backing.value("k").expect("persisted")).is_some());
});
