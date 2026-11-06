//! Stable-toolchain replay of the fuzz corpus plus named regression inputs.
//!
//! Every interesting fuzz input becomes a named test here and a file in
//! `windows/fuzz/corpus/<target>/`.
#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;

use common::invariants::{check_activity, check_credits, check_key};

fn replay(target: &str, check: fn(&[u8])) -> usize {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        check(&bytes);
        n += 1;
    }
    n
}

/// Every committed `/key` corpus input passes the invariants.
#[test]
fn test_corpus_openrouter_key_replays() {
    assert!(replay("openrouter_key", check_key) >= 3);
}

/// Every committed `/credits` corpus input passes the invariants.
#[test]
fn test_corpus_openrouter_credits_replays() {
    assert!(replay("openrouter_credits", check_credits) >= 2);
}

/// Every committed `/activity` corpus input passes the invariants.
#[test]
fn test_corpus_openrouter_activity_replays() {
    assert!(replay("openrouter_activity", check_activity) >= 2);
}

/// Degenerate and hostile inputs never panic any check.
#[test]
fn test_hostile_inputs_never_panic() {
    let inputs: [&[u8]; 14] = [
        b"",
        b"null",
        b"{}",
        b"[]",
        b"{\"data\":null}",
        b"{\"data\":[]}",
        b"{\"data\":{\"usage\":\"nan\"}}",
        b"{\"data\":{\"usage\":-1e999}}",
        b"{\"data\":{\"usage\":1,\"limit\":0,\"limit_reset\":\"\"}}",
        b"{\"data\":{\"total_credits\":1e308,\"total_usage\":-1e308}}",
        b"{\"data\":[{\"date\":1}]}",
        b"\xff\xfe\x00",
        b"{\"data\":{\"usage\":1,\"limit\":1e308,\"limit_remaining\":-1e308}}",
        b"{\"data\":[{\"date\":\"d\",\"model\":\"\",\"usage\":-3,\"requests\":-1,\"prompt_tokens\":1e30,\"completion_tokens\":1,\"reasoning_tokens\":1}]}",
    ];
    for i in inputs {
        check_key(i);
        check_credits(i);
        check_activity(i);
    }
}

/// Regression: overflowing sums of costs and token counts saturate instead of
/// producing infinity or wrapping negative.
#[test]
fn test_regression_activity_sum_overflow_saturates() {
    check_activity(
        br#"{"data":[{"date":"d","model":"m","usage":1e308,"requests":9223372036854775807,"prompt_tokens":9223372036854775807,"completion_tokens":9223372036854775807,"reasoning_tokens":0},{"date":"d","model":"m","usage":1e308,"requests":5,"prompt_tokens":5,"completion_tokens":5,"reasoning_tokens":0},{"date":"d","model":"n","usage":1e308,"requests":1,"prompt_tokens":1,"completion_tokens":1,"reasoning_tokens":0}]}"#,
    );
}

/// Regression: a limit smaller than a denormal-adjacent usage cannot make
/// the percent NaN or exceed 100.
#[test]
fn test_regression_extreme_limit_ratio_stays_in_range() {
    check_key(br#"{"data":{"usage":1e308,"limit":1e-308,"limit_remaining":-1e308,"limit_reset":"weekly"}}"#);
}
