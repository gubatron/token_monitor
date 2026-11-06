//! `ProviderUsagePoller` wiring for Grok on the core polling loop.
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{clock_at, utc};
use tokenmon_core::polling::testing::ManualScheduler;
use tokenmon_core::settings::Settings;
use tokenmon_core::{
    AuthEventListener, AuthSession, AuthSessionState, BoxFuture, ProviderError, ProviderId,
    ProviderUsagePoller, SessionGeneration,
};
use tokenmon_provider_grok::history::GrokHistoryStore;
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;
use tokenmon_provider_grok::poller::{GrokFetch, GrokUsagePoller};
use tokenmon_provider_grok::provider_poller::GrokProviderPoller;

struct FakeAuth(Mutex<AuthSessionState>);

impl AuthSession for FakeAuth {
    fn cookie_header(&self) -> Option<String> {
        Some("sso=live".to_owned())
    }
    fn state(&self) -> AuthSessionState {
        self.0.lock().unwrap().clone()
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        self.0.lock().unwrap().record_auth_failure(reason)
    }
    fn record_auth_success(&self) {
        self.0.lock().unwrap().record_auth_success();
    }
    fn is_current(&self, generation: SessionGeneration) -> bool {
        self.0.lock().unwrap().is_current(generation)
    }
    fn add_listener(&self, _listener: Arc<dyn AuthEventListener>) {}
}

struct CountingFetch(AtomicUsize);

impl GrokFetch for CountingFetch {
    fn fetch(
        &self,
        _cookie: String,
        _email: Option<String>,
    ) -> BoxFuture<'_, Result<WeeklyUsageSnapshot, ProviderError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(WeeklyUsageSnapshot::new(utc(2026, 7, 15, 12, 0), 42.0))
        })
    }
}

struct Rig {
    provider: GrokProviderPoller,
    settings: Arc<Mutex<Settings>>,
    fetch: Arc<CountingFetch>,
    scheduler: ManualScheduler,
}

fn rig(initial: Settings) -> Rig {
    let mut state = AuthSessionState::default();
    state.mark_signed_in(Some("me@x.co".to_owned()));
    let auth = Arc::new(FakeAuth(Mutex::new(state)));
    let fetch = Arc::new(CountingFetch(AtomicUsize::new(0)));
    let clock = clock_at(utc(2026, 7, 15, 12, 0));
    let inner = Arc::new(GrokUsagePoller::new(
        auth,
        GrokHistoryStore::in_memory(clock.clone()),
        clock,
        fetch.clone(),
        None,
    ));
    let settings = Arc::new(Mutex::new(initial));
    let source = {
        let settings = Arc::clone(&settings);
        Arc::new(move || settings.lock().unwrap().clone())
    };
    let scheduler = ManualScheduler::new();
    let sched = scheduler.clone();
    let provider = GrokProviderPoller::new(inner, source, move |b| {
        b.clock(sched.clock()).sleeper(Arc::new(sched.clone()))
    });
    Rig {
        provider,
        settings,
        fetch,
        scheduler,
    }
}

fn not_needed() -> Settings {
    Settings::default()
        .with_selected_provider(ProviderId::Cursor)
        .with_menu_bar_graph(ProviderId::Grok, false)
        .with_alert_providers(&std::collections::BTreeSet::new())
}

/// With the Cursor tab selected, no Grok menu-bar graph and no alert, the loop
/// is parked; turning the Grok graph on makes a refresh due at once.
#[tokio::test]
async fn test_loop_parks_while_grok_is_not_needed() {
    let r = rig(not_needed());
    assert_eq!(r.provider.polling_loop().delay_until_due(), None);
    {
        let mut s = r.settings.lock().unwrap();
        *s = s.clone().with_menu_bar_graph(ProviderId::Grok, true);
    }
    assert_eq!(
        r.provider.polling_loop().delay_until_due(),
        Some(Duration::ZERO)
    );
}

/// A manual refresh publishes through the loop and surfaces on the trait.
#[tokio::test]
async fn test_refresh_now_publishes_through_the_loop() {
    let r = rig(Settings::default().with_selected_provider(ProviderId::Grok));
    r.provider.refresh_now().await;
    assert_eq!(r.fetch.0.load(Ordering::SeqCst), 1);
    assert_eq!(r.provider.headline_used_percent(), Some(42.0));
    assert_eq!(r.provider.provider(), ProviderId::Grok);
    assert_eq!(r.provider.alert_account().as_deref(), Some("me@x.co"));
    r.provider.clear_snapshot();
    assert_eq!(r.provider.headline_used_percent(), None);
    let _ = &r.scheduler;
}

/// Opening the menu switches to the active interval immediately: the loop is
/// woken and re-arms its wait.
#[tokio::test]
async fn test_set_menu_open_wakes_the_loop() {
    let r = rig(Settings::default().with_selected_provider(ProviderId::Grok));
    r.provider.start();
    r.scheduler.settle().await;
    assert_eq!(r.fetch.0.load(Ordering::SeqCst), 1);
    r.provider.set_menu_open(true);
    r.scheduler.settle().await;
    assert!(r.scheduler.cancelled_count() >= 1);
    r.provider.stop();
}
