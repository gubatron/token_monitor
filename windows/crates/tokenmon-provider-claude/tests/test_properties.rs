//! Property tests and stable-toolchain regressions for the untrusted-input
//! parsers (mirrors the fuzz invariants in `windows/fuzz`).

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use proptest::prelude::*;
use tokenmon_provider_claude::ledger::{decode_ledger, encode_ledger};
use tokenmon_provider_claude::{
    organization_id_from_cookie_header, parse_complete_lines, parse_jsonl, parse_usage_body,
    usage_line_ranges,
};

proptest! {
    /// Arbitrary bytes: consumed is bounded, newline-aligned, and agrees with a prefix parse.
    #[test]
    fn prop_parse_complete_lines_invariants(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        let (entries, consumed) = parse_complete_lines(&data);
        prop_assert!(consumed <= data.len());
        prop_assert!(consumed == data.len() || consumed == 0 || data[consumed - 1] == b'\n');
        prop_assert_eq!(entries, parse_jsonl(&data[..consumed]));
        for r in usage_line_ranges(&data) {
            prop_assert!(r.end <= data.len() && !data[r].contains(&b'\n'));
        }
    }

    /// Lines mixing the usage marker with junk never panic.
    #[test]
    fn prop_marker_lines_never_panic(prefix in "[ -~]{0,40}", suffix in "[ -~]{0,40}") {
        let line = format!("{prefix}\"usage\"{suffix}\n");
        let _ = parse_complete_lines(line.as_bytes());
    }

    /// Usage windows are always within 0..=100.
    #[test]
    fn prop_usage_percents_clamped(u in any::<f64>(), p in any::<f64>()) {
        let body = format!(
            r#"{{"five_hour":{{"utilization":"{u}"}},"limits":[{{"group":"weekly","percent":"{p}"}}]}}"#
        );
        if let Ok(r) = parse_usage_body(body.as_bytes()) {
            for w in [r.five_hour, r.seven_day].into_iter().flatten() {
                prop_assert!((0.0..=100.0).contains(&w.used_percent));
            }
        }
    }

    /// Arbitrary bytes never panic the usage parser.
    #[test]
    fn prop_usage_body_never_panics(data in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = parse_usage_body(&data);
    }

    /// The org id is never empty.
    #[test]
    fn prop_org_id_never_empty(h in ".{0,80}") {
        if let Some(org) = organization_id_from_cookie_header(&h) {
            prop_assert!(!org.is_empty());
        }
    }

    /// Whatever decodes from a ledger file re-encodes to the same days.
    #[test]
    fn prop_ledger_roundtrip(data in proptest::collection::vec(any::<u8>(), 0..256)) {
        let days = decode_ledger(&data);
        prop_assert_eq!(decode_ledger(&encode_ledger(&days)), days);
    }
}

/// Regression: a token count of 1e30 once risked an `Int64` trap in Swift; it saturates here.
#[test]
fn test_regression_huge_token_count_saturates() {
    let line = br#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"usage":{"input_tokens":1e300}}}"#;
    assert_eq!(parse_jsonl(line)[0].input_tokens, i64::MAX);
}

/// Regression: saturating adds keep summed `i64::MAX` entries from overflowing.
#[test]
fn test_regression_summing_saturated_counts_does_not_overflow() {
    let one = r#"{"type":"assistant","timestamp":"2026-10-10T10:00:00Z","message":{"usage":{"input_tokens":1e300,"output_tokens":1e300}}}"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.jsonl"), format!("{one}\n{one}\n")).unwrap();
    let cache = tokenmon_provider_claude::ClaudeCodeFileCache::new();
    cache.update(dir.path());
    let days = cache.daily_tokens(tokenmon_provider_claude::DayCalendar::Utc);
    let t = days["2026-10-10"]["unknown"];
    assert_eq!(t.input_tokens, i64::MAX);
    assert_eq!(t.total_tokens(), i64::MAX);
}

/// Regression: `=x` and `name=` pairs are skipped like Swift's `split` does.
#[test]
fn test_regression_org_cookie_edge_pairs() {
    assert_eq!(
        organization_id_from_cookie_header("=x; lastActiveOrg=ok"),
        Some("ok".into())
    );
    assert_eq!(
        organization_id_from_cookie_header("lastActiveOrg=; lastActiveOrg=ok"),
        Some("ok".into())
    );
    assert_eq!(organization_id_from_cookie_header(";;;"), None);
}

/// Regression: deeply nested JSON does not overflow the stack.
#[test]
fn test_regression_deeply_nested_json() {
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(parse_usage_body(deep.as_bytes()).is_err());
    let line = format!("{{\"usage\":{deep}}}\n");
    assert!(parse_jsonl(line.as_bytes()).is_empty());
}
