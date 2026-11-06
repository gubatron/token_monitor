#![no_main]
//! claude.ai usage JSON/HTML and the org-id cookie parser.

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_claude::{organization_id_from_cookie_header, parse_usage_body};

fuzz_target!(|data: &[u8]| {
    if let Ok(r) = parse_usage_body(data) {
        for w in [r.five_hour, r.seven_day].into_iter().flatten() {
            assert!((0.0..=100.0).contains(&w.used_percent));
            assert!((0.0..=100.0).contains(&w.remaining_percent()));
        }
    }
    if let Ok(s) = std::str::from_utf8(data)
        && let Some(org) = organization_id_from_cookie_header(s)
    {
        assert!(!org.is_empty());
    }
});
