//! Invariants: body classification is total over arbitrary bytes (including
//! non-UTF-8); only `Unauthorized`/`BadResponse` errors come back; an
//! `Unauthorized` from `json_object` implies the body looks like HTML; status
//! mapping never panics for any status code.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::{ProviderError, ProviderErrorContext};
use tokenmon_provider_cursor::{
    RequestPolicy, json_object, looks_like_html, looks_like_login_page, map_status,
    reject_unauthorized_body,
};

fuzz_target!(|data: &[u8]| {
    let ctx = ProviderErrorContext::Cursor;
    match json_object(data, ctx) {
        Ok(_) | Err(ProviderError::BadResponse(..)) => {}
        Err(ProviderError::Unauthorized(_)) => assert!(looks_like_html(data)),
        Err(other) => panic!("unexpected error {other:?}"),
    }
    match reject_unauthorized_body(data, ctx) {
        Ok(_) | Err(ProviderError::Unauthorized(_) | ProviderError::BadResponse(..)) => {}
        Err(other) => panic!("unexpected error {other:?}"),
    }
    let _ = looks_like_login_page(data);
    let status = u16::from_le_bytes([
        data.first().copied().unwrap_or(0),
        data.get(1).copied().unwrap_or(0),
    ]);
    for policy in [RequestPolicy::CURSOR, RequestPolicy::GROKBOT] {
        let _ = map_status(status, data.len() % 2 == 0, Some(f64::NAN), data, policy);
    }
});
