//! Build-time repository setting parsing.

use tokenmon_core::update::feed::{DEFAULT_REPOSITORY, ReleaseSource, parse_repository};

/// Valid slugs parse; paths, extra segments and dot-dot are rejected.
#[test]
fn test_parse_repository_accepts_slugs_only() {
    assert_eq!(parse_repository("o/r"), Some(("o", "r")));
    for bad in [
        "", "o", "o/", "/r", "o/r/x", "../r", "o/..", "o r/x", "o/r?x",
    ] {
        assert_eq!(parse_repository(bad), None, "{bad}");
    }
}

/// The compiled-in default is itself a valid slug and feeds `ReleaseSource`.
#[test]
fn test_default_repository_is_valid_and_used() {
    assert!(parse_repository(DEFAULT_REPOSITORY).is_some());
    let s = ReleaseSource::default();
    assert!(
        s.releases_url()
            .contains(&format!("{}/{}", s.owner, s.repository))
    );
}
