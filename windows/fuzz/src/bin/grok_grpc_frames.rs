//! Invariants: framing, trailer parsing and status validation never panic on
//! arbitrary bytes; frame payloads lie inside the body; an unauthorized result
//! only comes from status 16 or an `unauthenticated` message.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::ProviderError;
use tokenmon_provider_grok::grpc::{
    data_frames, looks_like_protobuf, trailer_fields, validate_headers, validate_trailers,
};

fuzz_target!(|data: &[u8]| {
    for frame in data_frames(data) {
        assert!(frame.len() <= data.len());
    }
    let _ = looks_like_protobuf(data);

    let fields = trailer_fields(data);
    if let Err(ProviderError::Unauthorized(_)) = validate_trailers(data) {
        // Same integer grammar as `validate_status` (Swift `Int`): `+16` is 16.
        let status = fields
            .get("grpc-status")
            .and_then(|s| s.trim_matches([' ', '\t']).parse::<i64>().ok());
        let message = fields.get("grpc-message").map_or("", String::as_str);
        assert!(status == Some(16) || message.to_lowercase().contains("unauthenticated"));
    }

    // Header strings: split the input into a status and a message.
    let text = String::from_utf8_lossy(data);
    let (status, message) = text.split_once('\n').unwrap_or((&text, ""));
    let _ = validate_headers(Some(status), Some(message));
});
