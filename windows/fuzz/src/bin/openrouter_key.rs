#![no_main]
//! Fuzzes the OpenRouter `/key` decoder and snapshot builder.

use libfuzzer_sys::fuzz_target;

#[path = "../../crates/tokenmon-provider-openrouter/tests/common/invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| invariants::check_key(data));
