//! Invariants: the Cursor body check is total over arbitrary bytes (including
//! non-UTF-8); only `Unauthorized`/`BadResponse` errors come back; an
//! `Unauthorized` from the core `json_object` implies the body looks like
//! HTML. (Status mapping and challenge detection are core-net fuzz targets.)
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::net::{json_object, looks_like_html};
use tokenmon_core::{ProviderError, ProviderErrorContext};
use tokenmon_provider_cursor::reject_unauthorized_body;

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
});
