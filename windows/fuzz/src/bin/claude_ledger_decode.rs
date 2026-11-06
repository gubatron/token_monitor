#![no_main]
//! `claude_code_daily_tokens.dat`: bad input decodes to an empty ledger, good
//! input round-trips.

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_claude::ledger::{decode_ledger, encode_ledger, merge_days};

fuzz_target!(|data: &[u8]| {
    let days = decode_ledger(data);
    assert_eq!(decode_ledger(&encode_ledger(&days)), days);
    assert_eq!(merge_days(&days, &days), days);
});
