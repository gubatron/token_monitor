//! End-to-end install flow with a fake downloader and launcher (port of the
//! `AppInstallerTests` intent: failures leave the installed exe untouched and
//! leave no siblings behind).
#![allow(clippy::unwrap_used)]

mod common;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{fake_msi, fake_pe, make_zip, make_zip_with_symlink, sha};
use tokenmon_core::BoxFuture;
use tokenmon_core::update::{
    AppVersion, AssetKind, InstallError, InstallOutcome, InstallRequest, ReleaseAsset,
    ReleaseSource, UpdateInstaller,
};
use tokenmon_platform::install::{
    CommandSpec, DownloadedFile, Downloader, PeHeaderVerifier, ProcessLauncher,
    WindowsUpdateInstaller,
};

const URL: &str = "https://github.com/gubatron/token_monitor/releases/download/windows-v1.6.0/tokenmon-windows-x64.zip";

struct FakeDownloader {
    body: Vec<u8>,
    calls: AtomicUsize,
}
impl Downloader for FakeDownloader {
    fn download<'a>(
        &'a self,
        _url: &'a str,
        dest: &'a Path,
        max: u64,
    ) -> BoxFuture<'a, Result<DownloadedFile, InstallError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            assert_eq!(max, 80 * 1024 * 1024);
            fs::write(dest, &self.body).unwrap();
            Ok(DownloadedFile {
                path: dest.to_path_buf(),
                size: self.body.len() as u64,
                sha256: sha(&self.body),
            })
        })
    }
}

#[derive(Default)]
struct FakeLauncher {
    spawned: Mutex<Vec<CommandSpec>>,
    fail: bool,
}
impl ProcessLauncher for FakeLauncher {
    fn spawn_detached(&self, spec: &CommandSpec) -> io::Result<()> {
        self.spawned.lock().unwrap().push(spec.clone());
        if self.fail {
            Err(io::Error::other("boom"))
        } else {
            Ok(())
        }
    }
}

struct Rig {
    dir: tempfile::TempDir,
    exe: PathBuf,
    work: PathBuf,
    downloader: Arc<FakeDownloader>,
    launcher: Arc<FakeLauncher>,
    installer: WindowsUpdateInstaller,
}

fn rig(body: Vec<u8>, fail_launch: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    let work = dir.path().join("work");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&work).unwrap();
    let exe = app.join("TokenMon.exe");
    fs::write(&exe, fake_pe("old")).unwrap();
    let downloader = Arc::new(FakeDownloader {
        body,
        calls: AtomicUsize::new(0),
    });
    let launcher = Arc::new(FakeLauncher {
        fail: fail_launch,
        ..FakeLauncher::default()
    });
    let installer = WindowsUpdateInstaller::new(
        ReleaseSource::default(),
        exe.clone(),
        4242,
        work.clone(),
        downloader.clone(),
        Arc::new(PeHeaderVerifier),
        launcher.clone(),
    );
    Rig {
        dir,
        exe,
        work,
        downloader,
        launcher,
        installer,
    }
}

fn request(body: &[u8], kind: AssetKind, url: &str, current: &str, target: &str) -> InstallRequest {
    InstallRequest {
        asset: ReleaseAsset {
            name: "x".into(),
            url: url.into(),
            sha256: Some(sha(body)),
        },
        kind,
        expected_sha256: sha(body).to_uppercase(),
        current: AppVersion::parse(current).unwrap(),
        target: AppVersion::parse(target).unwrap(),
    }
}

fn zip_req(body: &[u8]) -> InstallRequest {
    request(body, AssetKind::Zip, URL, "1.5.0", "1.6.0")
}

fn siblings(r: &Rig) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(r.exe.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn assert_untouched(r: &Rig) {
    assert_eq!(fs::read(&r.exe).unwrap(), fake_pe("old"));
    assert_eq!(siblings(r), ["TokenMon.exe"]);
    assert_eq!(
        fs::read_dir(&r.work).unwrap().count(),
        0,
        "work dir cleaned"
    );
}

/// A verified zip replaces the exe in place, schedules the relaunch and cleans up.
#[tokio::test]
async fn test_installs_verified_update_in_place() {
    let body = make_zip(&[("TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), false);
    let out = r.installer.install(zip_req(&body)).await.unwrap();
    assert_eq!(out, InstallOutcome::Relaunching);
    assert_eq!(fs::read(&r.exe).unwrap(), fake_pe("new"));
    assert_eq!(siblings(&r), ["TokenMon.exe", "TokenMon.exe.old"]);
    assert_eq!(fs::read_dir(&r.work).unwrap().count(), 0);
    let spawned = r.launcher.spawned.lock().unwrap();
    assert_eq!(spawned[0].program, r.exe.as_os_str());
    assert_eq!(spawned[0].args, ["--relaunch-after-pid", "4242"]);
}

/// A failed relaunch still installs and asks for a manual restart.
#[tokio::test]
async fn test_launch_failure_asks_for_manual_restart() {
    let body = make_zip(&[("TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), true);
    assert_eq!(
        r.installer.install(zip_req(&body)).await.unwrap(),
        InstallOutcome::InstalledNeedsManualRestart
    );
    assert_eq!(fs::read(&r.exe).unwrap(), fake_pe("new"));
}

/// A checksum mismatch is refused and nothing changes.
#[tokio::test]
async fn test_checksum_mismatch_is_refused() {
    let body = make_zip(&[("TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), false);
    let mut req = zip_req(&body);
    req.expected_sha256 = "0".repeat(64);
    let e = r.installer.install(req).await.unwrap_err();
    assert_eq!(
        e,
        InstallError::Verification("The download does not match the release checksum.".into())
    );
    assert_untouched(&r);
    assert!(r.launcher.spawned.lock().unwrap().is_empty());
}

/// A non-PE exe is refused and nothing changes.
#[tokio::test]
async fn test_non_pe_exe_is_refused() {
    let body = make_zip(&[("TokenMon.exe", b"#!/bin/sh\nrm -rf /")]);
    let r = rig(body.clone(), false);
    assert!(matches!(
        r.installer.install(zip_req(&body)).await,
        Err(InstallError::Verification(_))
    ));
    assert_untouched(&r);
}

/// A symlink in the archive is refused and nothing changes.
#[tokio::test]
async fn test_symlink_in_archive_is_refused() {
    let body = make_zip_with_symlink();
    let r = rig(body.clone(), false);
    assert!(matches!(
        r.installer.install(zip_req(&body)).await,
        Err(InstallError::Archive(_))
    ));
    assert_untouched(&r);
}

/// A zip without a top-level exe is refused and nothing changes.
#[tokio::test]
async fn test_missing_exe_is_refused() {
    let body = make_zip(&[("nested/TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), false);
    assert!(matches!(
        r.installer.install(zip_req(&body)).await,
        Err(InstallError::Archive(_))
    ));
    assert_untouched(&r);
}

/// Downloads outside this repository's releases are refused before any request.
#[tokio::test]
async fn test_download_outside_repository_is_refused_without_request() {
    let body = make_zip(&[("TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), false);
    for url in [
        "https://evil.example/tokenmon-windows-x64.zip",
        "https://github.com/other/repo/releases/download/t/f.zip",
        "https://github.com/gubatron/token_monitor/releases/download/../x/y/f.zip",
    ] {
        let e = r
            .installer
            .install(request(&body, AssetKind::Zip, url, "1.5.0", "1.6.0"))
            .await
            .unwrap_err();
        assert_eq!(
            e,
            InstallError::Download(
                "The release file is not one of this project's GitHub releases.".into()
            )
        );
    }
    assert_eq!(r.downloader.calls.load(Ordering::SeqCst), 0);
    assert_untouched(&r);
}

/// Same version and downgrades are refused before any request.
#[tokio::test]
async fn test_same_version_and_downgrade_are_refused() {
    let body = make_zip(&[("TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), false);
    for target in ["1.5.0", "1.4.0"] {
        let e = r
            .installer
            .install(request(&body, AssetKind::Zip, URL, "1.5.0", target))
            .await
            .unwrap_err();
        assert_eq!(
            e,
            InstallError::Verification("The download is not newer than this version.".into())
        );
    }
    assert_eq!(r.downloader.calls.load(Ordering::SeqCst), 0);
}

/// An unwritable/missing exe location is refused without a request.
#[tokio::test]
async fn test_not_replaceable_is_refused_without_request() {
    let body = make_zip(&[("TokenMon.exe", &fake_pe("new"))]);
    let r = rig(body.clone(), false);
    fs::remove_file(&r.exe).unwrap();
    assert!(!r.installer.can_replace_running_app());
    assert_eq!(
        r.installer.install(zip_req(&body)).await.unwrap_err(),
        InstallError::NotWritable
    );
    assert_eq!(r.downloader.calls.load(Ordering::SeqCst), 0);
}

/// An MSI is verified and handed to msiexec; the package folder is kept.
#[tokio::test]
async fn test_msi_is_verified_and_launched() {
    let body = fake_msi();
    let r = rig(body.clone(), false);
    fs::remove_file(&r.exe).unwrap(); // MSI path does not need a replaceable exe
    let url = URL.replace(".zip", ".msi");
    let out = r
        .installer
        .install(request(&body, AssetKind::Msi, &url, "1.5.0", "1.6.0"))
        .await
        .unwrap();
    assert_eq!(out, InstallOutcome::Relaunching);
    let spawned = r.launcher.spawned.lock().unwrap();
    assert_eq!(spawned[0].program, "msiexec.exe");
    let msi = PathBuf::from(&spawned[0].args[1]);
    assert!(msi.starts_with(&r.work) && msi.is_file());
    drop(r.dir);
}

/// A file that is not an MSI is never launched.
#[tokio::test]
async fn test_non_msi_is_refused() {
    let body = b"MZ not an msi".to_vec();
    let r = rig(body.clone(), false);
    let url = URL.replace(".zip", ".msi");
    assert!(matches!(
        r.installer
            .install(request(&body, AssetKind::Msi, &url, "1.5.0", "1.6.0"))
            .await,
        Err(InstallError::Verification(_))
    ));
    assert!(r.launcher.spawned.lock().unwrap().is_empty());
}
