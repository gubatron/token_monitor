//! Ports of `AppVersionTests` (spec 01 section 3.2) plus ordering properties.
#![allow(clippy::unwrap_used)]

use proptest::prelude::*;
use tokenmon_core::update::AppVersion;

fn v(s: &str) -> AppVersion {
    AppVersion::parse(s).unwrap_or_else(|| panic!("{s} should parse"))
}

/// Plain, `v`-prefixed and two-component versions parse and print as written.
#[test]
fn test_parses_plain_and_tagged_versions() {
    assert_eq!(v("1.4.2").to_string(), "1.4.2");
    assert_eq!(v("v1.4.2").to_string(), "1.4.2");
    assert_eq!(v("V2.0").to_string(), "2.0");
}

/// Build metadata is dropped and does not affect ordering.
#[test]
fn test_parses_build_metadata() {
    assert_eq!(v("1.8.0+5").to_string(), "1.8.0");
    assert_eq!(
        v("v1.8.0-beta.1+exp.sha.5114f85").to_string(),
        "1.8.0-beta.1"
    );
    assert_eq!(v("1.8.0+5"), v("1.8.0"));
}

/// Release-name prefixes are stripped only when a version follows.
#[test]
fn test_parses_release_name_prefix() {
    assert_eq!(v("TokenMon-1.8.0").to_string(), "1.8.0");
    assert_eq!(v("TokenMon v1.8.0").to_string(), "1.8.0");
    assert_eq!(v("TokenMon-v1.8.0-beta.2").to_string(), "1.8.0-beta.2");
    assert!(AppVersion::parse("TokenMon-nightly").is_none());
    assert!(AppVersion::parse("TokenMon-").is_none());
}

/// Garbage is rejected rather than coerced.
#[test]
fn test_rejects_unparseable_tags() {
    for bad in [
        "nightly",
        "",
        "1.x.2",
        "   ",
        "v",
        "-beta",
        "1..2",
        "99999999999999999999999.1",
    ] {
        assert!(AppVersion::parse(bad).is_none(), "{bad:?} must be rejected");
    }
}

/// 1.10.0 sorts above 1.9.0.
#[test]
fn test_compares_numerically_not_lexically() {
    assert!(v("1.10.0") > v("1.9.0"));
    assert!(v("1.4.2") < v("1.4.10"));
    assert!(v("2.0.0") > v("1.99.99"));
}

/// Missing trailing components are zero.
#[test]
fn test_shorter_versions_pad_with_zero() {
    assert_eq!(v("1.4"), v("1.4.0"));
    assert!(v("1.4") < v("1.4.1"));
}

/// A prerelease sorts below its final release.
#[test]
fn test_prerelease_sorts_below_its_final_release() {
    assert!(v("1.5.0-beta.1") < v("1.5.0"));
    assert!(v("1.5.0-beta.1").is_prerelease());
    assert!(!v("1.5.0").is_prerelease());
}

/// Numeric identifiers compare numerically; numeric sorts before alphanumeric;
/// a shorter identifier list sorts first.
#[test]
fn test_prerelease_comparison_rules() {
    assert!(v("1.5.0-beta.2") < v("1.5.0-beta.10"));
    assert!(v("1.5.0-beta.1") < v("1.5.0-beta.2"));
    assert!(v("1.5.0-alpha.1") < v("1.5.0-beta.1"));
    assert!(v("1.5.0-1") < v("1.5.0-alpha"));
    assert!(v("1.5.0-beta") < v("1.5.0-beta.1"));
    assert!(v("1.5.0-18446744073709551616") < v("1.5.0-18446744073709551617"));
}

proptest! {
    /// Parsing never panics and ordering is antisymmetric on arbitrary text.
    #[test]
    fn prop_parse_total_and_ordering_antisymmetric(a in "\\PC{0,24}", b in "\\PC{0,24}") {
        if let (Some(x), Some(y)) = (AppVersion::parse(&a), AppVersion::parse(&b)) {
            prop_assert_eq!(x.cmp(&y), y.cmp(&x).reverse());
        }
    }

    /// Display output reparses to an equal version.
    #[test]
    fn prop_display_roundtrips(a in "[vV]?[0-9]{1,4}(\\.[0-9]{1,4}){0,3}(-[a-z0-9.]{1,8})?") {
        if let Some(x) = AppVersion::parse(&a) {
            let again = AppVersion::parse(&x.to_string());
            prop_assert_eq!(again, Some(x));
        }
    }
}
