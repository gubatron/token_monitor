//! Stable-toolchain replay of the update fuzz targets' invariants: every
//! committed corpus seed plus random bytes. Crash inputs found by the fuzzers
//! are added below as named regression tests.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use proptest::prelude::*;
use tokenmon_core::update::{
    AppVersion, ReleaseSource, is_repository_release_download, is_trusted_download, newer_release,
};

fn corpus(target: &str) -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| std::fs::read(e.path()).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

/// Same invariants as `fuzz/src/bin/update_app_version.rs`.
fn check_version(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (a, b) = text.split_once('\n').unwrap_or((text, text));
    let Some(x) = AppVersion::parse(a) else {
        return;
    };
    assert_eq!(AppVersion::parse(&x.to_string()), Some(x.clone()));
    if let Some(y) = AppVersion::parse(b) {
        assert_eq!(x.cmp(&y), y.cmp(&x).reverse());
    }
}

/// Same invariants as `fuzz/src/bin/update_release_feed.rs`.
fn check_feed(data: &[u8]) {
    let source = ReleaseSource::default();
    let current = AppVersion::parse("1.0.0").unwrap();
    let Ok(Some(r)) = newer_release(data, &current, &source) else {
        return;
    };
    assert!(r.version > current && !r.version.is_prerelease());
    for a in r.archive.iter().chain(r.installer.iter()) {
        assert!(is_repository_release_download(
            &a.url,
            &source.owner,
            &source.repository
        ));
        assert!(
            a.name
                .to_ascii_lowercase()
                .starts_with(&source.asset_prefix)
        );
    }
}

/// Same invariants as `fuzz/src/bin/update_trust.rs`.
fn check_trust(data: &[u8]) {
    let Ok(url) = std::str::from_utf8(data) else {
        return;
    };
    let repo = is_repository_release_download(url, "gubatron", "token_monitor");
    assert!(!repo || is_trusted_download(url, "gubatron", "token_monitor"));
    if repo {
        let rest = url.split_once("://").map_or("", |(_, r)| r);
        let segs: Vec<&str> = rest.split('/').skip(1).collect();
        assert_eq!(segs.len(), 6);
        assert!(
            segs.iter()
                .all(|s| !s.is_empty() && *s != "." && *s != "..")
        );
        assert!(!url.contains(['\\', '?', '#']));
        assert!(!rest.split('/').next().unwrap_or("").contains('@'));
    }
}

/// Every committed seed satisfies its target's invariants.
#[test]
fn test_corpus_seeds_satisfy_invariants() {
    for d in corpus("update_app_version") {
        check_version(&d);
    }
    for d in corpus("update_release_feed") {
        check_feed(&d);
    }
    for d in corpus("update_trust") {
        check_trust(&d);
    }
}

/// The sample windows release in the corpus really is offered (guards the seed).
#[test]
fn test_windows_release_seed_is_offered() {
    let seed = corpus("update_release_feed")
        .into_iter()
        .find(|d| String::from_utf8_lossy(d).contains("tokenmon-windows-x64.zip"))
        .unwrap();
    let r = newer_release(
        &seed,
        &AppVersion::parse("1.0.0").unwrap(),
        &ReleaseSource::default(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(r.archive.unwrap().name, "tokenmon-windows-x64.zip");
}

proptest! {
    /// Arbitrary bytes never violate the invariants or panic.
    #[test]
    fn prop_random_bytes(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        check_version(&data);
        check_feed(&data);
        check_trust(&data);
    }

    /// Arbitrary URL-ish text (slashes and dots weighted up) never passes the
    /// trust check with a dot segment.
    #[test]
    fn prop_urlish_text(s in "https://github\\.com/(gubatron|x)/(token_monitor|y)/releases/download/[a-z.%0-9/]{0,20}") {
        check_trust(s.as_bytes());
    }
}

/// Fuzz crash (issue #34): `1- +` kept a lone-space prerelease whose `Display`
/// (`1- `) reparsed with no prerelease.
#[test]
fn test_fuzz_regression_whitespace_prerelease_roundtrips() {
    use tokenmon_core::update::AppVersion;
    let v = AppVersion::parse("1- +").expect("parses");
    assert_eq!(v.prerelease(), None);
    assert_eq!(AppVersion::parse(&v.to_string()), Some(v));
    let w = AppVersion::parse("1- 0 .1\u{b}\t").expect("parses");
    assert_eq!(w.prerelease(), Some("0 .1"));
    assert_eq!(AppVersion::parse(&w.to_string()), Some(w));
}

/// Fuzz crash (issue #35): `@` in a release tag is legal and harmless; only an
/// `@` in the authority (userinfo) must be refused.
#[test]
fn test_fuzz_regression_at_sign_in_tag_is_not_userinfo() {
    use tokenmon_core::update::is_repository_release_download;
    let ok = "https://github.com/gubatron/token_monitor/releases/download/a~@~/top";
    assert!(is_repository_release_download(
        ok,
        "gubatron",
        "token_monitor"
    ));
    let userinfo = "https://github.com@evil.com/gubatron/token_monitor/releases/download/t/f";
    assert!(!is_repository_release_download(
        userinfo,
        "gubatron",
        "token_monitor"
    ));
}
