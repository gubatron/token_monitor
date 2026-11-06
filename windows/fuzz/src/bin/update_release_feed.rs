#![no_main]
//! `newer_release` never panics on arbitrary feed bytes, and anything it
//! offers is a strictly newer Windows release with only trusted assets.

use libfuzzer_sys::fuzz_target;
use tokenmon_core::update::{
    AppVersion, ReleaseSource, is_repository_release_download, is_repository_release_page,
    newer_release,
};

fuzz_target!(|data: &[u8]| {
    let source = ReleaseSource::default();
    let current = AppVersion::parse("1.0.0").expect("constant parses");
    let Ok(Some(release)) = newer_release(data, &current, &source) else {
        return;
    };
    assert!(release.version > current);
    assert!(!release.version.is_prerelease());
    assert!(
        release
            .tag
            .to_ascii_lowercase()
            .starts_with(&source.tag_prefix)
    );
    assert!(
        is_repository_release_page(&release.page_url, &source.owner, &source.repository)
            || release.page_url == source.releases_page_url()
    );
    for asset in release.archive.iter().chain(release.installer.iter()) {
        assert!(is_repository_release_download(
            &asset.url,
            &source.owner,
            &source.repository
        ));
        assert!(
            asset
                .name
                .to_ascii_lowercase()
                .starts_with(&source.asset_prefix)
        );
        if let Some(sha) = &asset.sha256 {
            assert_eq!(sha.len(), 64);
        }
    }
});
