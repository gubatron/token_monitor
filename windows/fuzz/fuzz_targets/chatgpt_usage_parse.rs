//! Invariants: neither parser panics; parsed percentages are finite and within
//! 0..=100; `display_plan_name`, window labels and the pool are total.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_chatgpt::{ChatGptSnapshot, parse_session_token, parse_usage};

fuzz_target!(|data: &[u8]| {
    let _ = parse_session_token(data);
    if let Ok(response) = parse_usage(data) {
        for w in [&response.primary, &response.secondary].into_iter().flatten() {
            assert!(w.used_percent.is_finite() && (0.0..=100.0).contains(&w.used_percent));
            let _ = w.label("Fallback");
            assert!((0.0..=100.0).contains(&w.remaining_percent()));
        }
        let snap = ChatGptSnapshot::new(response, chrono::DateTime::UNIX_EPOCH);
        assert!((0.0..=100.0).contains(&snap.headline_used_percent()));
        let _ = (snap.display_plan_name(), snap.usage_pool(), snap.resets_at());
    }
});
