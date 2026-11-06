//! Property tests for the untrusted-input parsers: totality and the
//! invariants the fuzz targets assert.
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

mod common;

use chrono::Utc;
use common::{FIXTURE_A_HEX, FIXTURE_B_HEX, hex};
use proptest::prelude::*;
use tokenmon_provider_grok::export::csv_escape;
use tokenmon_provider_grok::grpc::{
    data_frames, parse_usage, scan_protobuf, trailer_fields, validate_headers,
};
use tokenmon_provider_grok::models::ProductCatalog;
use tokenmon_provider_grok::rest::parse_json;

fn check_parsed(bytes: &[u8]) {
    if let Ok(parsed) = parse_usage(bytes, Utc::now()) {
        assert!((0.0..=100.0).contains(&parsed.used_percent));
        let mut seen = std::collections::HashSet::new();
        let mut last_index = 0;
        for p in &parsed.products {
            assert!(seen.insert(p.id.clone()), "duplicate product id");
            assert!(p.percent_of_pool > 0.05 && p.percent_of_pool <= 100.0);
            let index = ProductCatalog::display_index(&p.id);
            assert!(index >= last_index, "not in display order");
            last_index = index;
        }
    }
}

proptest! {
    /// `parse_usage` is total over arbitrary bytes and keeps its invariants.
    #[test]
    fn prop_parse_usage_total(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        check_parsed(&bytes);
    }

    /// Flipping bytes of a real response never breaks the invariants.
    #[test]
    fn prop_mutated_fixtures_keep_invariants(
        which in 0usize..2,
        edits in proptest::collection::vec((0usize..200, any::<u8>()), 0..8),
    ) {
        let mut bytes = hex(if which == 0 { FIXTURE_A_HEX } else { FIXTURE_B_HEX });
        for (at, value) in edits {
            let len = bytes.len();
            bytes[at % len] = value;
        }
        check_parsed(&bytes);
    }

    /// Frame walking, trailer parsing and the protobuf scan never panic, and
    /// scan paths stay within five fields.
    #[test]
    fn prop_frames_trailers_and_scan_total(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let frames = data_frames(&bytes);
        prop_assert!(frames.iter().all(|f| f.len() <= bytes.len()));
        let _ = trailer_fields(&bytes);
        let scan = scan_protobuf(&bytes, 0);
        prop_assert!(scan.fixed32.iter().all(|f| f.path.len() <= 5));
        prop_assert!(scan.varints.iter().all(|v| v.path.len() <= 5));
    }

    /// Header status parsing never panics on arbitrary strings.
    #[test]
    fn prop_validate_headers_total(status in ".{0,24}", message in ".{0,40}") {
        let _ = validate_headers(Some(&status), Some(&message));
    }

    /// The REST parser is total; a result stays within 0..=100.
    #[test]
    fn prop_rest_parser_total(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        if let Some(s) = parse_json(&bytes, None, Utc::now()) {
            prop_assert!((0.0..=100.0).contains(&s.used_percent));
            prop_assert!((0.0..=100.0).contains(&s.remaining_percent));
        }
    }

    /// CSV cells never start with a formula trigger (outside the `'` guard),
    /// and quoting is balanced.
    #[test]
    fn prop_csv_escape_is_a_safe_single_field(value in ".{0,40}") {
        let out = csv_escape(&value);
        let inner = out.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(&out);
        let first = inner.chars().next();
        prop_assert!(!matches!(first, Some('=' | '+' | '-' | '@' | '\t' | '\r')));
        if out.starts_with('"') {
            prop_assert!(out.len() >= 2);
            prop_assert_eq!(inner.replace("\"\"", "").matches('"').count(), 0);
        } else {
            prop_assert!(!out.contains([',', '"', '\n', '\r']));
        }
    }
}
