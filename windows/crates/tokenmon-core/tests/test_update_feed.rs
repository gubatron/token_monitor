//! Ports of `ReleaseFeedTests` (spec 01 section 3.3), adapted to Windows tags
//! and asset names, plus macOS-asset-never-selected and list-feed cases.
#![allow(clippy::unwrap_used)]

use tokenmon_core::update::{
    AppVersion, MAX_FEED_BYTES, ReleaseSource, UpdateError, newer_release, sha256_from_digest,
};

const DL: &str = "https://github.com/gubatron/token_monitor/releases/download";

fn cur(s: &str) -> AppVersion {
    AppVersion::parse(s).expect("version")
}

fn check(
    body: &str,
    current: &str,
) -> Result<Option<tokenmon_core::update::AvailableRelease>, UpdateError> {
    newer_release(body.as_bytes(), &cur(current), &ReleaseSource::default())
}

fn release_with_assets(tag: &str, assets: &str) -> String {
    format!(
        r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,
        "html_url":"https://github.com/gubatron/token_monitor/releases/tag/{tag}",
        "published_at":"2026-11-01T12:00:00Z","assets":[{assets}]}}"#
    )
}

fn asset(tag: &str, name: &str, digest: Option<&str>) -> String {
    let d = digest.map_or_else(String::new, |d| format!(r#","digest":"{d}""#));
    format!(r#"{{"name":"{name}","browser_download_url":"{DL}/{tag}/{name}"{d}}}"#)
}

/// A strictly newer windows-v tag is reported with its fields.
#[test]
fn test_reports_newer_release() {
    let r = check(&release_with_assets("windows-v1.6.0", ""), "1.5.0")
        .unwrap()
        .unwrap();
    assert_eq!(r.version.to_string(), "1.6.0");
    assert_eq!(r.tag, "windows-v1.6.0");
    assert!(r.page_url.ends_with("/releases/tag/windows-v1.6.0"));
    assert!(r.published_at.is_some());
}

/// Same or older versions are not updates.
#[test]
fn test_same_or_older_version_is_not_an_update() {
    assert!(
        check(&release_with_assets("windows-v1.5.0", ""), "1.5.0")
            .unwrap()
            .is_none()
    );
    assert!(
        check(&release_with_assets("windows-v1.4.0", ""), "1.5.0")
            .unwrap()
            .is_none()
    );
}

/// Drafts and prereleases are ignored, including prerelease tags.
#[test]
fn test_drafts_and_prereleases_are_ignored() {
    for body in [
        r#"{"tag_name":"windows-v9.9.9","draft":true}"#,
        r#"{"tag_name":"windows-v9.9.9","prerelease":true}"#,
        r#"{"tag_name":"windows-v9.9.9-beta.1"}"#,
    ] {
        assert!(check(body, "1.5.0").unwrap().is_none(), "{body}");
    }
}

/// A missing `html_url` falls back to the releases page.
#[test]
fn test_missing_page_url_falls_back_to_releases_page() {
    let r = check(r#"{"tag_name":"windows-v2.0.0"}"#, "1.0.0")
        .unwrap()
        .unwrap();
    assert_eq!(
        r.page_url,
        "https://github.com/gubatron/token_monitor/releases/latest"
    );
}

/// A macOS tag is not ours: no update and no error.
#[test]
fn test_non_windows_tag_is_ignored() {
    assert!(
        check(r#"{"tag_name":"v9.0.0"}"#, "1.0.0")
            .unwrap()
            .is_none()
    );
    assert!(
        check(r#"{"tag_name":"TokenMon-9.0.0"}"#, "1.0.0")
            .unwrap()
            .is_none()
    );
}

/// An unparseable windows tag is an error.
#[test]
fn test_unparseable_tag_throws() {
    let e = check(r#"{"tag_name":"windows-vnightly"}"#, "1.0.0").unwrap_err();
    assert_eq!(
        e,
        UpdateError::BadResponse("Release carries no usable version tag".into())
    );
    assert!(check(r#"{"draft":false}"#, "1.0.0").is_err());
}

/// Non-object payloads are rejected.
#[test]
fn test_unexpected_payloads_are_rejected() {
    for body in ["[1,2]", "42", "null", "not json", ""] {
        let r = check(body, "1.0.0");
        // arrays of junk are skipped, scalars and garbage are errors
        if body == "[1,2]" {
            assert!(r.unwrap().is_none());
        } else {
            assert_eq!(
                r.unwrap_err(),
                UpdateError::BadResponse("Unexpected release payload".into())
            );
        }
    }
}

/// Oversized payloads are refused before parsing.
#[test]
fn test_oversized_payload_is_rejected() {
    let big = vec![b' '; MAX_FEED_BYTES + 1];
    assert!(newer_release(&big, &cur("1.0.0"), &ReleaseSource::default()).is_err());
}

/// The Windows zip and msi are selected; the macOS zip never is.
#[test]
fn test_windows_assets_selected_and_macos_zip_never() {
    let tag = "windows-v1.6.0";
    let assets = [
        asset(
            tag,
            "TokenMon-1.6.0.zip",
            Some(&format!("sha256:{}", "a".repeat(64))),
        ),
        asset(tag, "TokenMon-1.6.0.pkg", None),
        asset(tag, "tokenmon-windows-x64-pdb.zip", None),
        asset(
            tag,
            "tokenmon-windows-x64.zip",
            Some(&format!("sha256:{}", "AB".repeat(32))),
        ),
        asset(tag, "tokenmon-windows-x64.msi", None),
    ]
    .join(",");
    let r = check(&release_with_assets(tag, &assets), "1.5.0")
        .unwrap()
        .unwrap();
    let zip = r.archive.unwrap();
    assert_eq!(zip.name, "tokenmon-windows-x64.zip");
    assert_eq!(zip.sha256.as_deref(), Some("ab".repeat(32).as_str()));
    assert_eq!(r.installer.unwrap().name, "tokenmon-windows-x64.msi");
}

/// With only macOS assets nothing is selected.
#[test]
fn test_only_macos_assets_yield_no_asset() {
    let tag = "windows-v1.6.0";
    let assets = [
        asset(tag, "TokenMon-1.6.0.zip", None),
        asset(tag, "TokenMon-1.6.0-dSYM.zip", None),
    ]
    .join(",");
    let r = check(&release_with_assets(tag, &assets), "1.5.0")
        .unwrap()
        .unwrap();
    assert!(r.archive.is_none() && r.installer.is_none());
}

/// Assets from outside the repository's release downloads are dropped.
#[test]
fn test_untrusted_asset_is_ignored() {
    let body = r#"{"tag_name":"windows-v1.6.0","assets":[
      {"name":"tokenmon-windows-x64.zip","browser_download_url":"https://evil.example/tokenmon-windows-x64.zip"},
      {"name":"tokenmon-windows-arm64.zip","browser_download_url":"https://github.com/gubatron/token_monitor/releases/download/../../x/y/z"},
      {"name":"tokenmon-windows-x64.msi","browser_download_url":"https://github.com/other/token_monitor/releases/download/t/tokenmon-windows-x64.msi"}]}"#;
    let r = check(body, "1.5.0").unwrap().unwrap();
    assert!(r.archive.is_none() && r.installer.is_none());
}

/// Digests: uppercase hex is lowercased; malformed ones are dropped.
#[test]
fn test_digest_parsing() {
    let ok = "AB".repeat(32);
    assert_eq!(
        sha256_from_digest(&format!("sha256:{ok}")),
        Some(ok.to_lowercase())
    );
    assert_eq!(
        sha256_from_digest(&format!("SHA256:{ok}")),
        Some(ok.to_lowercase())
    );
    for bad in [
        "sha256:abc",
        "sha1:".to_owned().as_str(),
        "nocolon",
        "",
        &format!("sha256:{}", "g".repeat(64)),
        &format!("md5:{ok}"),
    ] {
        assert_eq!(sha256_from_digest(bad), None, "{bad}");
    }
}

/// From a release list, the newest eligible Windows release wins; mac releases
/// and malformed entries are skipped.
#[test]
fn test_list_feed_picks_newest_windows_release() {
    let body = format!(
        "[{},{},{},{},42]",
        r#"{"tag_name":"v9.0.0"}"#,
        r#"{"tag_name":"windows-v1.7.0","draft":true}"#,
        release_with_assets("windows-v1.6.0", ""),
        release_with_assets("windows-v1.5.5", ""),
    );
    let r = check(&body, "1.5.0").unwrap().unwrap();
    assert_eq!(r.version.to_string(), "1.6.0");
}
