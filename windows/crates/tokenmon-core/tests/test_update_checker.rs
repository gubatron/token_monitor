//! Ports of `UpdateCheckerTests` (spec 01 sections 3.4 and 3.5) with fakes.
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokenmon_core::BoxFuture;
use tokenmon_core::update::{
    AppVersion, InstallError, InstallOutcome, InstallRequest, ReleaseFeedFetcher, ReleaseSource,
    UpdateChecker, UpdateError, UpdateInstaller, UpdateSettings, UrlOpener,
};

const DL: &str = "https://github.com/gubatron/token_monitor/releases/download/windows-v1.6.0";

struct Feed {
    response: Mutex<Result<Vec<u8>, UpdateError>>,
    requests: AtomicUsize,
}
impl Feed {
    fn ok(body: &str) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Ok(body.as_bytes().to_vec())),
            requests: AtomicUsize::new(0),
        })
    }
    fn err(e: UpdateError) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Err(e)),
            requests: AtomicUsize::new(0),
        })
    }
}
impl ReleaseFeedFetcher for Feed {
    fn fetch(&self) -> BoxFuture<'_, Result<Vec<u8>, UpdateError>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let r = self.response.lock().unwrap().clone();
        Box::pin(async move { r })
    }
}

struct Settings(AtomicBool);
impl UpdateSettings for Settings {
    fn checks_for_updates(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Opener(Mutex<Vec<String>>);
impl UrlOpener for Opener {
    fn open(&self, url: &str) {
        self.0.lock().unwrap().push(url.to_owned());
    }
}

struct FakeInstaller {
    can_replace: bool,
    result: Mutex<Option<Result<InstallOutcome, InstallError>>>,
    requests: Mutex<Vec<InstallRequest>>,
}
impl FakeInstaller {
    fn new(can_replace: bool, result: Result<InstallOutcome, InstallError>) -> Self {
        Self {
            can_replace,
            result: Mutex::new(Some(result)),
            requests: Mutex::default(),
        }
    }
}
impl UpdateInstaller for FakeInstaller {
    fn can_replace_running_app(&self) -> bool {
        self.can_replace
    }
    fn install(
        &self,
        request: InstallRequest,
    ) -> BoxFuture<'_, Result<InstallOutcome, InstallError>> {
        self.requests.lock().unwrap().push(request);
        let r = self.result.lock().unwrap().take().expect("one install");
        Box::pin(async move { r })
    }
}

fn make(
    feed: &Arc<Feed>,
    current: Option<&str>,
    auto: bool,
) -> (UpdateChecker, Arc<Opener>, Arc<Settings>) {
    let opener = Arc::new(Opener::default());
    let settings = Arc::new(Settings(AtomicBool::new(auto)));
    let c = UpdateChecker::new(
        ReleaseSource::default(),
        current.and_then(AppVersion::parse),
        settings.clone(),
        feed.clone(),
        opener.clone(),
    );
    (c, opener, settings)
}

fn release(assets: &str) -> String {
    format!(
        r#"{{"tag_name":"windows-v1.6.0","html_url":"https://github.com/gubatron/token_monitor/releases/tag/windows-v1.6.0","assets":[{assets}]}}"#
    )
}

fn asset(name: &str, digest: bool) -> String {
    let d = if digest {
        format!(r#","digest":"sha256:{}""#, "c".repeat(64))
    } else {
        String::new()
    };
    format!(r#"{{"name":"{name}","browser_download_url":"{DL}/{name}"{d}}}"#)
}

/// A newer release is published into the state.
#[tokio::test]
async fn test_newer_release_is_published() {
    let feed = Feed::ok(&release(""));
    let (c, ..) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let s = c.state();
    assert_eq!(s.available_release.unwrap().version.to_string(), "1.6.0");
    assert!(s.last_error.is_none() && !s.is_checking);
}

/// Same release is ignored.
#[tokio::test]
async fn test_same_release_is_ignored() {
    let feed = Feed::ok(&release(""));
    let (c, ..) = make(&feed, Some("1.6.0"), true);
    c.check_now().await;
    assert!(c.state().available_release.is_none());
}

/// 403 reports the friendly rate-limit message; other statuses the code.
#[tokio::test]
async fn test_rate_limit_and_server_error_messages() {
    let (c, ..) = make(
        &Feed::err(UpdateError::from_status(403)),
        Some("1.5.0"),
        true,
    );
    c.check_now().await;
    assert_eq!(
        c.state().last_error.as_deref(),
        Some("Update check failed: GitHub rate limit reached; will retry later")
    );
    let (c, ..) = make(
        &Feed::err(UpdateError::from_status(500)),
        Some("1.5.0"),
        true,
    );
    c.check_now().await;
    assert_eq!(
        c.state().last_error.as_deref(),
        Some("Update check failed: HTTP 500")
    );
}

/// A build without a version makes no request and sets no error.
#[tokio::test]
async fn test_missing_version_skips_check() {
    let feed = Feed::ok(&release(""));
    let (c, ..) = make(&feed, None, true);
    c.check_now().await;
    assert_eq!(feed.requests.load(Ordering::SeqCst), 0);
    assert!(c.state().available_release.is_none() && c.state().last_error.is_none());
    c.check_manually().await;
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("This build has no version number to compare.")
    );
    assert_eq!(feed.requests.load(Ordering::SeqCst), 0);
}

/// A manual check runs with automatic checks off and says so when current.
#[tokio::test]
async fn test_manual_check_runs_when_automatic_checks_are_off() {
    let feed = Feed::ok(&release(""));
    let (c, ..) = make(&feed, Some("1.6.0"), false);
    c.check_manually().await;
    assert_eq!(feed.requests.load(Ordering::SeqCst), 1);
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("You're up to date.")
    );
}

/// A manual check surfaces a newer release and the action title.
#[tokio::test]
async fn test_manual_check_surfaces_newer_release() {
    let feed = Feed::ok(&release(&asset("tokenmon-windows-x64.zip", true)));
    let (c, ..) = make(&feed, Some("1.5.0"), true);
    assert_eq!(c.state().action_title(), "Check for Updates");
    c.check_manually().await;
    let s = c.state();
    assert_eq!(s.action_title(), "Update to 1.6.0…");
    assert!(s.status_message.is_none() && s.can_act());
}

/// With automatic checks off a background check makes no request.
#[tokio::test]
async fn test_checks_disabled_skips_request() {
    let feed = Feed::ok(&release(""));
    let (c, ..) = make(&feed, Some("1.5.0"), false);
    c.check_now().await;
    assert_eq!(feed.requests.load(Ordering::SeqCst), 0);
}

/// Turning the setting off clears state and tells the caller to stop.
#[tokio::test]
async fn test_settings_changed_off_clears_state() {
    let feed = Feed::ok(&release(""));
    let (c, _, settings) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    assert!(c.settings_changed());
    settings.0.store(false, Ordering::SeqCst);
    assert!(!c.settings_changed());
    let s = c.state();
    assert!(s.available_release.is_none() && s.last_error.is_none() && s.status_message.is_none());
}

/// Zip is installed when the exe can replace itself; the request carries the digest.
#[tokio::test]
async fn test_install_uses_zip_and_quits_for_relaunch() {
    let feed = Feed::ok(&release(&format!(
        "{},{}",
        asset("tokenmon-windows-x64.zip", true),
        asset("tokenmon-windows-x64.msi", true)
    )));
    let (c, opener, _) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(true, Ok(InstallOutcome::Relaunching));
    assert!(c.perform_primary_action(&inst).await);
    let reqs = inst.requests.lock().unwrap();
    assert_eq!(reqs[0].asset.name, "tokenmon-windows-x64.zip");
    assert_eq!(reqs[0].expected_sha256, "c".repeat(64));
    assert!(opener.0.lock().unwrap().is_empty());
    assert!(!c.state().is_installing);
}

/// A copy that cannot replace itself is handed the MSI.
#[tokio::test]
async fn test_not_replaceable_uses_msi() {
    let feed = Feed::ok(&release(&format!(
        "{},{}",
        asset("tokenmon-windows-x64.zip", true),
        asset("tokenmon-windows-x64.msi", true)
    )));
    let (c, ..) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(false, Ok(InstallOutcome::Relaunching));
    c.install_available_update(&inst).await;
    assert_eq!(
        inst.requests.lock().unwrap()[0].asset.name,
        "tokenmon-windows-x64.msi"
    );
}

/// Not replaceable and no MSI: the release page opens with an explanation.
#[tokio::test]
async fn test_not_replaceable_without_msi_opens_page() {
    let feed = Feed::ok(&release(&asset("tokenmon-windows-x64.zip", true)));
    let (c, opener, _) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(false, Ok(InstallOutcome::Relaunching));
    assert!(!c.install_available_update(&inst).await);
    assert!(inst.requests.lock().unwrap().is_empty());
    assert_eq!(opener.0.lock().unwrap().len(), 1);
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("TokenMon cannot replace itself from this folder.")
    );
}

/// No Windows asset at all opens the release page.
#[tokio::test]
async fn test_no_asset_opens_release_page() {
    let feed = Feed::ok(&release(&asset("TokenMon-1.6.0.zip", true)));
    let (c, opener, _) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(true, Ok(InstallOutcome::Relaunching));
    c.install_available_update(&inst).await;
    assert!(inst.requests.lock().unwrap().is_empty());
    assert_eq!(
        opener.0.lock().unwrap()[0],
        "https://github.com/gubatron/token_monitor/releases/tag/windows-v1.6.0"
    );
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("No installer was published — opening the release page.")
    );
}

/// Without a digest nothing is downloaded and the page opens instead.
#[tokio::test]
async fn test_missing_digest_opens_release_page_instead_of_installing() {
    let feed = Feed::ok(&release(&asset("tokenmon-windows-x64.zip", false)));
    let (c, opener, _) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(true, Ok(InstallOutcome::Relaunching));
    c.install_available_update(&inst).await;
    assert!(inst.requests.lock().unwrap().is_empty());
    assert_eq!(opener.0.lock().unwrap().len(), 1);
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("The release has no checksum to verify — opening the release page.")
    );
}

/// Installer errors surface as status text and open the release page.
#[tokio::test]
async fn test_install_error_opens_page_with_message() {
    let feed = Feed::ok(&release(&asset("tokenmon-windows-x64.zip", true)));
    let (c, opener, _) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(
        true,
        Err(InstallError::Verification(
            "The download does not match the release checksum.".into(),
        )),
    );
    assert!(!c.install_available_update(&inst).await);
    assert_eq!(opener.0.lock().unwrap().len(), 1);
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("The update failed verification: The download does not match the release checksum.")
    );
    assert!(!c.state().is_installing);
}

/// A failed relaunch asks the user to restart.
#[tokio::test]
async fn test_manual_restart_message() {
    let feed = Feed::ok(&release(&asset("tokenmon-windows-x64.zip", true)));
    let (c, ..) = make(&feed, Some("1.5.0"), true);
    c.check_now().await;
    let inst = FakeInstaller::new(true, Ok(InstallOutcome::InstalledNeedsManualRestart));
    assert!(!c.install_available_update(&inst).await);
    assert_eq!(
        c.state().status_message.as_deref(),
        Some("TokenMon 1.6.0 is installed. Quit and reopen TokenMon to finish.")
    );
}
