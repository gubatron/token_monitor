//! Invariants: JWT claim decoding never panics on any token string (bad
//! base64, huge or NaN-like `exp`); `exp` is finite; `valid_until` never
//! overflows and never exceeds `now + 30 min`.
#![no_main]

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_provider_chatgpt::jwt::{account_id, base64_url_decode, expiry_secs};
use tokenmon_provider_chatgpt::valid_until;

fuzz_target!(|data: &[u8]| {
    let token = String::from_utf8_lossy(data);
    let _ = account_id(&token);
    let _ = base64_url_decode(&token);
    if let Some(exp) = expiry_secs(&token) {
        assert!(exp.is_finite());
    }
    let now = Utc.timestamp_opt(1_790_000_000, 0).unwrap();
    assert!(valid_until(&token, now) <= now + chrono::Duration::seconds(1800));
});
