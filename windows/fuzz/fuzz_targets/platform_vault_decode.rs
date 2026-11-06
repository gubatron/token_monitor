#![no_main]
//! Decrypted vault JSON and pending-removals decoders: never panic, and a
//! decoded payload re-encodes to something that decodes to the same value.

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::secrets::{decode_payload, decode_pending, encode_payload};

fuzz_target!(|data: &[u8]| {
    if let Ok(p) = decode_payload(data) {
        let again = decode_payload(&encode_payload(&p)).expect("re-encoded payload decodes");
        assert_eq!(again, p);
    }
    let _ = decode_pending(data);
});
