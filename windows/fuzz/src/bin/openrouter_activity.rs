#![no_main]
//! Fuzzes the OpenRouter `/activity` decoder and per-model grouping.

use libfuzzer_sys::fuzz_target;

#[path = "../../../crates/tokenmon-provider-openrouter/tests/common/invariants.rs"]
mod invariants;

fuzz_target!(|data: &[u8]| invariants::check_activity(data));
