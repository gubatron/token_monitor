//! The Settings window's backend and host against the real app and the
//! platform crate's in-memory registry (spec 01 section 2.1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::FixedOffset;
use tokenmon_app::app::{App, AppInputs};
use tokenmon_app::settings_host::{AppHost, AppSettingsBackend, HostParts, refresh_time};
use tokenmon_app::sources::HistoryAccess;
use tokenmon_app::updates::UpdateService;
use tokenmon_core::BoxFuture;
use tokenmon_core::ProviderId;
use tokenmon_core::SystemClock;
use tokenmon_core::alerts::RecordingNotifier;
use tokenmon_core::storage::AppPaths;
use tokenmon_core::update::{
    AppVersion, InstallError, InstallOutcome, InstallRequest, ReleaseFeedFetcher, ReleaseSource,
    UpdateChecker, UpdateError, UpdateInstaller, UpdateSettings, UrlOpener,
};
use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, FakeEncryptor};
use tokenmon_platform::shell::{DesktopVerb, MemoryRegistry, RunKeyLoginItem};
use tokenmon_platform::webauth::FakeWebAuthHost;
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;
use tokenmon_provider_opencode::db::DataDirEnv;
use tokenmon_ui::settings::{
    ExportFormat, ProductCategory, RefreshStatus, SettingsBackend, SettingsChange, SettingsHost,
};

struct NoOpener;
impl UrlOpener for NoOpener {
    fn open(&self, _url: &str) {}
}

struct FakeHistory(std::sync::Mutex<Vec<WeeklyUsageSnapshot>>);
impl HistoryAccess for FakeHistory {
    fn snapshots(&self) -> Vec<WeeklyUsageSnapshot> {
        self.0.lock().expect("lock").clone()
    }
    fn clear(&self) {
        self.0.lock().expect("lock").clear();
    }
    fn store_failed(&self) -> bool {
        false
    }
    fn is_dirty(&self) -> bool {
        false
    }
    fn flush(&self) {}
}

const EXE: &str = r"C:\Program Files\TokenMon\TokenMon.exe";

struct Fixture {
    dir: tempfile::TempDir,
    app: App,
    host: AppHost,
    registry: Arc<MemoryRegistry>,
    history: Arc<FakeHistory>,
    quit: Arc<AtomicBool>,
}

fn fixture() -> Fixture {
    fixture_with(None)
}

/// A fixture whose update service is `updates` (production wiring when `None`).
fn fixture_with(updates: Option<Arc<UpdateService>>) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = App::build(
        AppInputs {
            paths: AppPaths::new(dir.path()),
            encryptor: Arc::new(FakeEncryptor::default()),
            fallback: APP_FALLBACK_POLICY,
            web_host: Arc::new(FakeWebAuthHost::default()),
            notifier: Arc::new(RecordingNotifier::default()),
            clock: Arc::new(SystemClock),
            version: "1.2.3".to_owned(),
            home: Some(dir.path().to_path_buf()),
            opencode_env: DataDirEnv::default(),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("builds");
    let registry = Arc::new(MemoryRegistry::new());
    let history = Arc::new(FakeHistory(std::sync::Mutex::default()));
    let quit = Arc::new(AtomicBool::new(false));
    let q = Arc::clone(&quit);
    let updates = updates.unwrap_or_else(|| {
        Arc::new(
            UpdateService::production(&app.settings, "1.2.3", EXE.into(), Arc::new(NoOpener))
                .expect("updates"),
        )
    });
    let host = AppHost::new(HostParts {
        settings: Arc::clone(&app.settings),
        model: Arc::clone(&app.model),
        sessions: Arc::clone(&app.sessions),
        signin: Arc::clone(&app.signin),
        updates,
        login_item: Arc::new(RunKeyLoginItem::new(registry.clone(), EXE)),
        desktop_verb: Arc::new(DesktopVerb::new(registry.clone(), EXE)),
        history: history.clone(),
        version: "1.2.3".to_owned(),
        tz: Arc::new(|| FixedOffset::east_opt(0).expect("utc")),
        handle: tokio::runtime::Handle::current(),
        quit: Arc::new(move || q.store(true, Ordering::SeqCst)),
    });
    Fixture {
        dir,
        app,
        host,
        registry,
        history,
        quit,
    }
}

/// Edits go through to the persisted store and the snapshot reports the
/// clamped, persisted truth.
#[tokio::test]
async fn test_backend_applies_and_reads_back_changes() {
    let f = fixture();
    let backend = AppSettingsBackend::new(Arc::clone(&f.app.settings));
    backend.apply(SettingsChange::ActivePollSeconds(120));
    backend.apply(SettingsChange::ThresholdEnabled(true));
    backend.apply(SettingsChange::ThresholdPercent(95.0));
    backend.apply(SettingsChange::AlertProvider(ProviderId::Claude, false));
    backend.apply(SettingsChange::MenuBarGraph(ProviderId::Cursor, true));
    backend.apply(SettingsChange::ProductVisible(
        ProductCategory::Voice,
        false,
    ));
    backend.apply(SettingsChange::MoveProvider {
        moving: ProviderId::OpenRouter,
        target: ProviderId::Grok,
    });
    let s = backend.snapshot();
    assert_eq!(s.active_poll_seconds, 120);
    assert!(s.threshold_enabled && (s.threshold_percent - 95.0).abs() < f64::EPSILON);
    assert!(!s.alert_providers.contains(&ProviderId::Claude));
    assert!(s.menu_bar_graph_providers.contains(&ProviderId::Cursor));
    assert!(!s.visible_products.contains(&ProductCategory::Voice));
    assert_eq!(s.provider_order[0], ProviderId::OpenRouter);
}

/// Disabling every provider is refused: the last one stays enabled.
#[tokio::test]
async fn test_backend_never_disables_the_last_provider() {
    let f = fixture();
    let backend = AppSettingsBackend::new(Arc::clone(&f.app.settings));
    for p in ProviderId::USAGE_PROVIDERS {
        backend.apply(SettingsChange::ProviderEnabled(p, false));
    }
    assert_eq!(backend.snapshot().enabled_providers.len(), 1);
}

/// Launch at login and the desktop verb write and remove their registry
/// entries and report back through the status.
#[tokio::test]
async fn test_host_toggles_autostart_and_desktop_verb() {
    let f = fixture();
    assert!(!f.host.status().launch_at_login);
    assert!(!f.host.status().desktop_menu_enabled);
    f.host.set_launch_at_login(true).expect("register");
    f.host.set_desktop_menu(true).expect("install");
    let status = f.host.status();
    assert!(status.launch_at_login && status.desktop_menu_enabled);
    assert_ne!(f.registry.keys().len(), 0);
    f.host.set_launch_at_login(false).expect("unregister");
    f.host.set_desktop_menu(false).expect("uninstall");
    let status = f.host.status();
    assert!(!status.launch_at_login && !status.desktop_menu_enabled);
}

/// A failing registry surfaces as the message the window shows.
#[tokio::test]
async fn test_host_reports_registry_write_failures() {
    let f = fixture();
    f.registry.set_fail_writes(true);
    assert!(f.host.set_launch_at_login(true).is_err());
}

/// Status carries the version, the update button and five account rows, all
/// signed out on a fresh install.
#[tokio::test]
async fn test_host_status_describes_a_fresh_install() {
    let f = fixture();
    let status = f.host.status();
    assert_eq!(status.app_version.as_deref(), Some("1.2.3"));
    assert_eq!(status.update_action_title, "Check for Updates");
    assert!(status.update_can_act);
    assert_eq!(status.accounts.len(), 5);
    assert!(status.accounts.iter().all(|a| !a.signed_in));
    assert!(!status.openrouter.connected);
    assert_eq!(status.refresh, RefreshStatus::None);
}

/// A bad OpenRouter key is refused with the session's message; a good one
/// connects, and signing out disconnects.
#[tokio::test]
async fn test_host_openrouter_key_round_trip() {
    let f = fixture();
    assert!(f.host.save_openrouter_key("hello").is_err());
    assert!(f.host.save_openrouter_key("sk-or-v1-abc").is_ok());
    assert!(f.host.status().openrouter.connected);
    f.host.sign_out_openrouter();
    assert!(!f.host.status().openrouter.connected);
}

/// Export writes the history in the chosen format; clearing empties it;
/// quit reaches the app.
#[tokio::test]
async fn test_host_exports_clears_and_quits() {
    let f = fixture();
    f.history
        .0
        .lock()
        .expect("lock")
        .push(WeeklyUsageSnapshot::new(chrono::Utc::now(), 12.5));
    let out = f.dir.path().join("grok.csv");
    f.host
        .export_history(ExportFormat::Csv, &out)
        .expect("export");
    let text = std::fs::read_to_string(&out).expect("read");
    assert!(text.starts_with("fetchedAt,usedPercent"));
    assert_eq!(text.lines().count(), 2);
    let json = f.dir.path().join("grok.json");
    f.host
        .export_history(ExportFormat::Json, &json)
        .expect("json");
    assert!(
        std::fs::read_to_string(&json)
            .expect("read")
            .contains("12.5")
    );
    f.host.clear_history();
    assert_eq!(f.history.snapshots().len(), 0);
    f.host.quit();
    assert!(f.quit.load(Ordering::SeqCst));
}

/// An unwritable destination is an error message, not a panic.
#[tokio::test]
async fn test_host_export_to_a_missingdirectory_fails_cleanly() {
    let f = fixture();
    let bad = f.dir.path().join("no").join("such").join("dir.csv");
    assert!(f.host.export_history(ExportFormat::Csv, &bad).is_err());
}

/// Refresh lines use the abbreviated date and shortened time.
#[test]
fn test_refresh_time_format() {
    use chrono::TimeZone;
    let at = chrono::Utc.with_ymd_and_hms(2026, 7, 10, 15, 4, 0).unwrap();
    assert_eq!(
        refresh_time(at, FixedOffset::east_opt(0).unwrap()),
        "Jul 10, 2026 at 3:04 PM"
    );
}

const RELEASE_URL: &str =
    "https://github.com/gubatron/token_monitor/releases/download/windows-v1.6.0";

struct OneRelease;
impl ReleaseFeedFetcher for OneRelease {
    fn fetch(&self) -> BoxFuture<'_, Result<Vec<u8>, UpdateError>> {
        let body = format!(
            r#"{{"tag_name":"windows-v1.6.0","html_url":"https://github.com/gubatron/token_monitor/releases/tag/windows-v1.6.0","assets":[{{"name":"tokenmon-windows-x64.zip","browser_download_url":"{RELEASE_URL}/tokenmon-windows-x64.zip","digest":"sha256:{}"}}]}}"#,
            "c".repeat(64)
        );
        Box::pin(async move { Ok(body.into_bytes()) })
    }
}

struct AlwaysOn;
impl UpdateSettings for AlwaysOn {
    fn checks_for_updates(&self) -> bool {
        true
    }
}

struct FakeInstaller(InstallOutcome);
impl UpdateInstaller for FakeInstaller {
    fn can_replace_running_app(&self) -> bool {
        true
    }
    fn install(
        &self,
        _request: InstallRequest,
    ) -> BoxFuture<'_, Result<InstallOutcome, InstallError>> {
        let outcome = self.0;
        Box::pin(async move { Ok(outcome) })
    }
}

/// A service that already found 1.6.0 and installs through `installer`.
async fn service_with_update(installer: FakeInstaller) -> Arc<UpdateService> {
    let checker = Arc::new(UpdateChecker::new(
        ReleaseSource::default(),
        AppVersion::parse("1.5.0"),
        Arc::new(AlwaysOn),
        Arc::new(OneRelease),
        Arc::new(NoOpener),
    ));
    checker.check_manually().await;
    assert!(checker.state().available_release.is_some());
    Arc::new(UpdateService {
        checker,
        installer: Arc::new(installer),
    })
}

async fn wait_until(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if done() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    done()
}

/// Installing an update that schedules a relaunch quits the app, so the
/// relaunched build is not handed off to a still-running instance (#63).
#[tokio::test]
async fn test_installing_an_update_quits_the_running_app() {
    let f = fixture_with(Some(
        service_with_update(FakeInstaller(InstallOutcome::Relaunching)).await,
    ));
    f.host.update_primary_action();
    assert!(wait_until(|| f.quit.load(Ordering::SeqCst)).await);
}

/// An install that needs a manual restart must not quit the app behind the
/// user's back.
#[tokio::test]
async fn test_manual_restart_install_keeps_the_app_running() {
    let f = fixture_with(Some(
        service_with_update(FakeInstaller(InstallOutcome::InstalledNeedsManualRestart)).await,
    ));
    f.host.update_primary_action();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert!(!f.quit.load(Ordering::SeqCst));
}
