//! Invariants: the REST usage parser never panics or overflows the stack on
//! deep wrapper nesting; on `Some`, used and remaining are within 0..=100 and
//! every product percent is clamped.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_grok::rest::parse_json;

fuzz_target!(|data: &[u8]| {
    let now = chrono::DateTime::UNIX_EPOCH;
    if let Some(snapshot) = parse_json(data, None, now) {
        assert!((0.0..=100.0).contains(&snapshot.used_percent));
        assert!((0.0..=100.0).contains(&snapshot.remaining_percent));
        assert!(
            snapshot
                .products
                .iter()
                .all(|p| (0.0..=100.0).contains(&p.percent_of_pool))
        );
    }
});
