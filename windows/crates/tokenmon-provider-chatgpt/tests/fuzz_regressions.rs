//! Stable-toolchain replay of the fuzz corpus and of every hostile input class
//! the fuzz targets cover (`fuzz/src/bin/chatgpt_*.rs`), asserting the
//! same invariants.
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]

mod common;

use chrono::{TimeZone, Utc};
use tokenmon_provider_chatgpt::jwt::{account_id, base64_url_decode, expiry_secs};
use tokenmon_provider_chatgpt::{ChatGptSnapshot, parse_session_token, parse_usage, valid_until};

fn check_usage(data: &[u8]) {
    let _ = parse_session_token(data);
    if let Ok(r) = parse_usage(data) {
        for w in [&r.primary, &r.secondary].into_iter().flatten() {
            assert!(w.used_percent.is_finite() && (0.0..=100.0).contains(&w.used_percent));
            let _ = w.label("Fallback");
        }
        let s = ChatGptSnapshot::new(r, chrono::DateTime::UNIX_EPOCH);
        assert!((0.0..=100.0).contains(&s.headline_used_percent()));
        let _ = (s.display_plan_name(), s.usage_pool(), s.resets_at());
    }
}

fn check_token(token: &str) {
    let _ = account_id(token);
    let _ = base64_url_decode(token);
    if let Some(e) = expiry_secs(token) {
        assert!(e.is_finite());
    }
    let now = Utc.timestamp_opt(1_790_000_000, 0).unwrap();
    assert!(valid_until(token, now) <= now + chrono::Duration::seconds(1800));
}

/// Every committed corpus file keeps satisfying the target invariants.
#[test]
fn test_corpus_replays_cleanly() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/corpus");
    for (dir, usage) in [("chatgpt_usage_parse", true), ("chatgpt_jwt_claims", false)] {
        let entries = std::fs::read_dir(format!("{root}/{dir}")).expect("corpus dir exists");
        for entry in entries {
            let bytes = std::fs::read(entry.unwrap().path()).unwrap();
            if usage {
                check_usage(&bytes);
            } else {
                check_token(&String::from_utf8_lossy(&bytes));
            }
        }
    }
}

/// Usage payloads with non-finite, huge, negative and wrongly typed numbers.
#[test]
fn test_usage_hostile_numbers() {
    for body in [
        r#"{"primary_window":{"used_percent":"nan"}}"#,
        r#"{"primary_window":{"used_percent":"1e400"}}"#,
        r#"{"primary_window":{"used_percent":-5,"limit_window_seconds":-1e300,"reset_at":1e300}}"#,
        r#"{"primary_window":{"used_percent":1,"limit_window_seconds":"9223372036854775808"}}"#,
        r#"{"primary_window":{"used_percent":1,"reset_at":"not a date"}}"#,
        r#"{"plan_type":"","rate_limit":[]}"#,
        r#"{"plan_type":"ǆ"}"#,
    ] {
        check_usage(body.as_bytes());
    }
    check_usage(&[0xff, 0xfe, b'{']);
    check_usage("[".repeat(10_000).as_bytes());
}

/// Tokens with bad base64, odd segment counts, duplicate keys and huge `exp`.
#[test]
fn test_token_hostile_inputs() {
    let dup = common::jwt(
        r#"{"exp":1,"exp":2,"https://api.openai.com/auth":{"a_account_id":"x","a_account_id":"y"}}"#,
    );
    check_token(&dup);
    assert_eq!(account_id(&dup).as_deref(), Some("y"));
    assert_eq!(expiry_secs(&dup), Some(2.0));
    for t in [
        "",
        ".",
        "..",
        "a.",
        ".a",
        "a.b",
        "a.=.c",
        "a.====.c",
        "a.é.c",
        "a.-_-_.c",
        "a.eyJleHAiOiJuYW4ifQ.c",
    ] {
        check_token(t);
    }
    for exp in [
        "1e308",
        "-1e308",
        "1.7976931348623157e308",
        "4e12",
        "-8e12",
        "8.1e12",
    ] {
        check_token(&common::jwt(&format!(r#"{{"exp":{exp}}}"#)));
    }
    check_token(&format!(
        "a.{}.c",
        common::b64url("[".repeat(10_000).as_bytes())
    ));
}
