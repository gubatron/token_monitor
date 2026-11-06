//! URL trust rules (spec 01 section 3.3), including raw-path `..` handling.
#![allow(clippy::unwrap_used)]

use tokenmon_core::update::{
    is_repository_release_download as repo, is_trusted_download as trusted,
};

const O: &str = "gubatron";
const R: &str = "token_monitor";

fn base(tail: &str) -> String {
    format!("https://github.com/{O}/{R}/releases/download/{tail}")
}

/// Plain repository downloads are accepted, owner/repo case-insensitively.
#[test]
fn test_repository_download_accepts_expected_shapes() {
    assert!(repo(&base("windows-v1.0.0/tokenmon-windows-x64.zip"), O, R));
    assert!(repo(
        "HTTPS://GitHub.com/GUBATRON/Token_Monitor/releases/download/t/f.zip",
        O,
        R
    ));
    assert!(repo(
        "https://github.com:443/gubatron/token_monitor/releases/download/t/f.zip",
        O,
        R
    ));
}

/// Everything that deviates from the exact shape is refused.
#[test]
fn test_repository_download_rejects_deviations() {
    let bad = [
        "http://github.com/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://evil.com/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://github.com.evil.com/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://user@github.com/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://user:pw@github.com/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://github.com:8443/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://github.com/other/token_monitor/releases/download/t/f.zip".to_owned(),
        "https://github.com/gubatron/other/releases/download/t/f.zip".to_owned(),
        "https://github.com/gubatron/token_monitor/Releases/download/t/f.zip".to_owned(),
        "https://github.com/gubatron/token_monitor/releases/Download/t/f.zip".to_owned(),
        "https://github.com/gubatron/token_monitor/releases/download/t".to_owned(),
        "https://github.com/gubatron/token_monitor/releases/download/t/x/f.zip".to_owned(),
        "https://github.com/gubatron/token_monitor/releases/download/t/f.zip?x=1".to_owned(),
        "https://github.com/gubatron/token_monitor/releases/download/t/f.zip#x".to_owned(),
        base("t//f.zip"),
        base("t/"),
        base("../f.zip"),
        base("./f.zip"),
        base("t/.."),
        base("%2e%2e/f.zip"),
        base("t/%2E%2e"),
        "https://github.com/gubatron/token_monitor/releases/download/../../x/y/z".to_owned(),
        "https://github.com\\@evil.com/gubatron/token_monitor/releases/download/t/f.zip".to_owned(),
        format!("{} ", base("t/f.zip")),
        String::new(),
        "https://".to_owned(),
    ];
    for url in bad {
        assert!(!repo(&url, O, R), "{url:?} must be refused");
    }
}

/// The CDN hosts are trusted (with query strings) but are not repository downloads.
#[test]
fn test_trusted_download_allows_cdn_hosts_only() {
    let cdn = "https://objects.githubusercontent.com/github-production/abc?X-Amz-Signature=1";
    let cdn2 = "https://release-assets.githubusercontent.com/x/y";
    assert!(trusted(cdn, O, R));
    assert!(trusted(cdn2, O, R));
    assert!(!repo(cdn, O, R));
    assert!(trusted(&base("t/f.zip"), O, R));
    for bad in [
        "http://objects.githubusercontent.com/a",
        "https://objects.githubusercontent.com.evil.com/a",
        "https://evil.com/objects.githubusercontent.com",
        "https://user@objects.githubusercontent.com/a",
        "https://objects.githubusercontent.com:444/a",
        "https://objects.githubusercontent.com/a/../b",
        "https://raw.githubusercontent.com/a/b",
    ] {
        assert!(!trusted(bad, O, R), "{bad:?} must be refused");
    }
}

/// #40: release pages of the configured repository only.
#[test]
fn test_release_page_allowlist() {
    use tokenmon_core::update::is_repository_release_page as ok;
    let (o, r) = ("gubatron", "token_monitor");
    assert!(ok(
        "https://github.com/gubatron/token_monitor/releases",
        o,
        r
    ));
    assert!(ok(
        "https://github.com/Gubatron/Token_Monitor/releases/latest",
        o,
        r
    ));
    assert!(ok(
        "https://github.com/gubatron/token_monitor/releases/tag/windows-v1.0.0",
        o,
        r
    ));
    for bad in [
        "",
        "file:///C:/x",
        "ms-msdt:/x",
        "http://github.com/gubatron/token_monitor/releases",
        "https://github.com/gubatron/token_monitor",
        "https://github.com/gubatron/token_monitor/Releases",
        "https://github.com/gubatron/token_monitor/releases//x",
        "https://github.com/gubatron/token_monitor/releases/%2e%2e/x",
        "https://github.com/gubatron/token_monitor/releases#x",
        "https://user@github.com/gubatron/token_monitor/releases",
        "https://github.com:8443/gubatron/token_monitor/releases",
        "https://github.com/gubatron/token_monitor_evil/releases",
        "https://evil.com/gubatron/token_monitor/releases",
    ] {
        assert!(!ok(bad, o, r), "{bad}");
    }
}
