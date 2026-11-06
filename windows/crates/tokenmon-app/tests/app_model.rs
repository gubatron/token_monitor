//! Lifecycle behaviour of the app model against fake pollers (spec 01
//! sections 6.2 to 6.7 and the `AppModelPollingTests` parity rows).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokenmon_app::alerts::AlertWatcher;
use tokenmon_app::model::{AppModel, CHANGE_COALESCING, MENU_OPEN_FRESHNESS};
use tokenmon_app::registry::{ProviderEntry, ProviderRegistry, SessionKey};
use tokenmon_app::smoke::run_refresh_cycle;
use tokenmon_app::testing::{FakePoller, PollerCall};
use tokenmon_core::ProviderId;
use tokenmon_core::alerts::{RecordingNotifier, ThresholdNotifier};
use tokenmon_core::settings::{AppSettings, SettingsFile};

struct Fixture {
    model: Arc<AppModel>,
    pollers: Vec<Arc<FakePoller>>,
    settings: Arc<AppSettings>,
}

impl Fixture {
    fn poller(&self, p: ProviderId) -> &Arc<FakePoller> {
        self.pollers
            .iter()
            .find(|f| provider_of(f) == p)
            .expect("poller registered")
    }
}

fn provider_of(p: &Arc<FakePoller>) -> ProviderId {
    use tokenmon_core::ProviderUsagePoller;
    p.provider()
}

/// Seven fakes; Grokbot shares Cursor's session like the real registry.
fn fixture() -> Fixture {
    let cursor_session = Arc::new(0_u8);
    let mut sessions = vec![Arc::clone(&cursor_session)];
    let pollers: Vec<Arc<FakePoller>> = ProviderId::USAGE_PROVIDERS
        .iter()
        .map(|p| FakePoller::new(*p))
        .collect();
    let entries = pollers
        .iter()
        .map(|f| {
            let provider = provider_of(f);
            let session = if provider.sign_in_provider() == ProviderId::Cursor {
                SessionKey::of(&cursor_session)
            } else {
                {
                    // Distinct, live allocations: a key is an address.
                    let own = Arc::new(0_u8);
                    sessions.push(Arc::clone(&own));
                    SessionKey::of(&own)
                }
            };
            ProviderEntry {
                provider,
                poller: Arc::clone(f) as _,
                session,
            }
        })
        .collect();
    let settings = Arc::new(AppSettings::new(Arc::new(SettingsFile::in_memory())));
    std::mem::forget(sessions);
    let model = AppModel::new(
        ProviderRegistry::new(entries),
        Arc::clone(&settings),
        tokio::runtime::Handle::current(),
    );
    Fixture {
        model,
        pollers,
        settings,
    }
}

/// Lets spawned refresh tasks run.
async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

/// `testMenuOpenStateReachesEveryPoller`.
#[tokio::test]
async fn test_menu_open_state_reaches_every_poller() {
    let f = fixture();
    f.model.set_menu_open(true);
    assert!(f.pollers.iter().all(|p| p.menu_is_open()));
    f.model.set_menu_open(false);
    assert!(f.pollers.iter().all(|p| !p.menu_is_open()));
}

/// `testSessionSharingFollowsTheRegistry`.
#[tokio::test]
async fn test_session_sharing_follows_the_registry() {
    let f = fixture();
    let reg = f.model.registry();
    assert_eq!(reg.sharing_session_with(ProviderId::Cursor).len(), 2);
    assert_eq!(reg.sharing_session_with(ProviderId::Grokbot).len(), 2);
    assert_eq!(reg.sharing_session_with(ProviderId::Claude).len(), 1);
}

/// `testSignInWindowFollowsTheSession`.
#[test]
fn test_sign_in_window_follows_the_session() {
    assert_eq!(ProviderId::Grokbot.sign_in_provider(), ProviderId::Cursor);
    for p in ProviderId::USAGE_PROVIDERS
        .into_iter()
        .filter(|p| *p != ProviderId::Grokbot)
    {
        assert_eq!(p.sign_in_provider(), p);
    }
}

/// The registry is in `ProviderId` order whatever order it is built in.
#[tokio::test]
async fn test_registry_is_in_provider_order() {
    let f = fixture();
    let order: Vec<_> = f
        .model
        .registry()
        .entries()
        .iter()
        .map(|e| e.provider)
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted);
}

/// Opening the panel refreshes every provider that needs polling, with the
/// 15 s freshness rule; closing refreshes nothing.
#[tokio::test]
async fn test_menu_open_requests_a_fresh_refresh_only_when_opening() {
    let f = fixture();
    f.model.set_menu_open(false);
    settle().await;
    assert!(f.pollers.iter().all(|p| p.refreshes() == 0));

    f.model.set_menu_open(true);
    settle().await;
    let snapshot = f.settings.snapshot();
    for p in &f.pollers {
        let provider = provider_of(p);
        let expected = usize::from(snapshot.needs_polling(provider));
        assert_eq!(
            p.count(PollerCall::RefreshIfOlder(MENU_OPEN_FRESHNESS)),
            expected,
            "{provider}"
        );
    }
}

/// A disabled provider is not refreshed on menu open.
#[tokio::test]
async fn test_menu_open_skips_providers_that_do_not_need_polling() {
    let f = fixture();
    let enabled = std::collections::BTreeSet::from([ProviderId::Claude]);
    f.settings
        .set_enabled_providers(&enabled)
        .expect("non-empty set");
    f.model.set_menu_open(true);
    settle().await;
    assert_eq!(f.poller(ProviderId::Claude).refreshes(), 1);
    assert_eq!(f.poller(ProviderId::Grok).refreshes(), 0);
}

/// A Cursor sign-in refreshes Cursor and Grokbot, nobody else.
#[tokio::test]
async fn test_sign_in_completed_refreshes_every_poller_sharing_the_session() {
    let f = fixture();
    for t in f.model.sign_in_completed(ProviderId::Cursor) {
        t.await.expect("refresh task");
    }
    assert_eq!(f.poller(ProviderId::Cursor).refreshes(), 1);
    assert_eq!(f.poller(ProviderId::Grokbot).refreshes(), 1);
    assert_eq!(f.poller(ProviderId::Claude).refreshes(), 0);
}

/// Sleep pauses every loop, wake resumes every loop, and neither forces a
/// refresh by itself.
#[tokio::test]
async fn test_sleep_pauses_and_ready_resumes_every_loop() {
    let f = fixture();
    f.model.pause_all();
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Pause) == 1));
    f.model.resume_all();
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Resume) == 1));
    assert!(f.pollers.iter().all(|p| p.refreshes() == 0));
}

/// Starting wires every loop once; quitting stops them once and runs the
/// flush hooks exactly once even if quit is called twice.
#[tokio::test]
async fn test_quit_stops_loops_and_flushes_stores_once() {
    let f = fixture();
    let flushed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&flushed);
    f.model.on_quit(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    f.model.start_all();
    f.model.quit();
    f.model.quit();
    assert!(f.model.has_quit());
    assert_eq!(flushed.load(Ordering::SeqCst), 1);
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Start) == 1));
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Stop) == 1));
}

/// Changing the poll cadence wakes every loop once, after coalescing.
#[tokio::test(start_paused = true)]
async fn test_polling_input_change_wakes_every_loop() {
    let f = fixture();
    let _watch = f.model.watch_settings();
    settle().await;
    f.settings.set_active_poll_seconds(60);
    f.settings.set_idle_poll_seconds(120);
    tokio::time::sleep(CHANGE_COALESCING * 3).await;
    settle().await;
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Wake) == 1));
}

/// A settings change that does not affect polling wakes nothing.
#[tokio::test(start_paused = true)]
async fn test_unrelated_setting_change_does_not_wake_loops() {
    let f = fixture();
    let _watch = f.model.watch_settings();
    settle().await;
    f.settings.set_checks_for_updates(false);
    tokio::time::sleep(CHANGE_COALESCING * 3).await;
    settle().await;
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Wake) == 0));
}

/// Showing a provider's graph in the tray refreshes it immediately.
#[tokio::test(start_paused = true)]
async fn test_newly_graphed_provider_refreshes_now() {
    let f = fixture();
    let _watch = f.model.watch_settings();
    settle().await;
    f.settings.set_menu_bar_graph(ProviderId::Claude, true);
    tokio::time::sleep(CHANGE_COALESCING * 3).await;
    settle().await;
    assert_eq!(
        f.poller(ProviderId::Claude).count(PollerCall::RefreshNow),
        1
    );
    assert_eq!(
        f.poller(ProviderId::Cursor).count(PollerCall::RefreshNow),
        0
    );
}

/// The alert watcher notifies once when a headline crosses the threshold.
#[tokio::test]
async fn test_alert_watcher_notifies_once_over_threshold() {
    let f = fixture();
    f.settings.set_threshold_enabled(true);
    f.settings.set_threshold_percent(80.0);
    f.settings
        .set_alert_providers(&std::collections::BTreeSet::from([ProviderId::Claude]));
    f.poller(ProviderId::Claude).set_headline(85.0, None);
    let sink = Arc::new(RecordingNotifier::default());
    let watcher = AlertWatcher::new(
        f.model.registry().clone(),
        Arc::clone(&f.settings),
        ThresholdNotifier::new(f.settings.file(), sink.clone()),
    );
    watcher.evaluate_all();
    watcher.evaluate_all();
    let alerts = sink.alerts();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].provider, ProviderId::Claude);
}

/// The smoke cycle refreshes everything, quits, and passes.
#[tokio::test]
async fn test_smoke_cycle_refreshes_every_provider_and_quits() {
    let f = fixture();
    let report = run_refresh_cycle(&f.model, Duration::from_secs(5)).await;
    assert!(report.passed());
    assert!(f.pollers.iter().all(|p| p.refreshes() == 1));
    assert!(f.model.has_quit());
}

/// A change made right after `watch_settings()` returns, before its task has
/// run once, is still handled (the starting value is read before spawning).
#[tokio::test(start_paused = true)]
async fn test_change_before_the_watch_task_first_runs_is_not_lost() {
    let f = fixture();
    let _watch = f.model.watch_settings();
    f.settings.set_active_poll_seconds(77);
    tokio::time::sleep(CHANGE_COALESCING * 3).await;
    settle().await;
    assert!(f.pollers.iter().all(|p| p.count(PollerCall::Wake) == 1));
}

/// A panel that vanished without the shell hiding it must not leave the
/// pollers on the active cadence.
#[tokio::test(start_paused = true)]
async fn test_sync_menu_open_follows_panel_visibility() {
    let f = fixture();
    assert!(!f.model.sync_menu_open(false), "already closed");
    assert!(f.model.sync_menu_open(true));
    assert!(f.model.is_menu_open());
    assert!(!f.model.sync_menu_open(true), "unchanged");
    assert!(f.model.sync_menu_open(false), "window hidden another way");
    assert!(!f.model.is_menu_open());
    assert!(f.pollers.iter().all(|p| !p.menu_is_open()));
}
