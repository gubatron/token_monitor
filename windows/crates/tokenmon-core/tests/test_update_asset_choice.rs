//! Asset selection for the install flow: an MSI-installed app updates via the
//! MSI so Windows Installer's records stay correct.
#![allow(clippy::unwrap_used)]

use tokenmon_core::update::{AppVersion, AssetKind, AvailableRelease, ReleaseAsset, choose_asset};

fn asset(name: &str) -> ReleaseAsset {
    ReleaseAsset {
        name: name.to_owned(),
        url: format!(
            "https://github.com/gubatron/token_monitor/releases/download/windows-v1.6.0/{name}"
        ),
        sha256: Some("0".repeat(64)),
    }
}

fn release(zip: bool, msi: bool) -> AvailableRelease {
    AvailableRelease {
        version: AppVersion::parse("1.6.0").unwrap(),
        tag: "windows-v1.6.0".to_owned(),
        page_url: String::new(),
        archive: zip.then(|| asset("tokenmon-windows-1.6.0.zip")),
        installer: msi.then(|| asset("tokenmon-windows-1.6.0.msi")),
        published_at: None,
    }
}

fn kind(r: &AvailableRelease, can_replace: bool, by_msi: bool) -> Option<AssetKind> {
    choose_asset(r, can_replace, by_msi).map(|(_, k)| k)
}

/// Both assets: a zip install keeps preferring the zip, an MSI install the MSI,
/// even when the exe is writable in place (the MSI folder is user-writable).
#[test]
fn test_msi_install_prefers_msi_even_when_replaceable() {
    let r = release(true, true);
    assert_eq!(kind(&r, true, false), Some(AssetKind::Zip));
    assert_eq!(kind(&r, true, true), Some(AssetKind::Msi));
    assert_eq!(kind(&r, false, false), Some(AssetKind::Msi));
    assert_eq!(kind(&r, false, true), Some(AssetKind::Msi));
}

/// With only a zip published, an MSI install falls back to it only when it can
/// be replaced in place; with only an MSI, every case uses the MSI.
#[test]
fn test_single_asset_fallbacks() {
    let zip_only = release(true, false);
    assert_eq!(kind(&zip_only, true, true), Some(AssetKind::Zip));
    assert_eq!(kind(&zip_only, false, true), None);
    assert_eq!(kind(&zip_only, false, false), None);
    let msi_only = release(false, true);
    for (can, by) in [(true, false), (true, true), (false, false), (false, true)] {
        assert_eq!(kind(&msi_only, can, by), Some(AssetKind::Msi));
    }
    assert_eq!(kind(&release(false, false), true, true), None);
}
