//! `OpenCodeUsagePoller` against a scripted console, a fake cookie session and
//! an injected scan (ports of `OpenCodePollerFallbackTests` and the OpenCode
//! rows of `PollerSessionGuardTests`).
#![allow(clippy::unwrap_used, clippy::unused_async, clippy::assert_is_empty)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::settings::{AppSettings, SettingsFile};
use tokenmon_core::usage::WeekOffset;
use tokenmon_core::{
    AUTH_FAILURE_THRESHOLD, AuthEvent, AuthEventListener, AuthSession, AuthSessionState,
    ManualClock, ProviderError, ProviderErrorContext, ProviderId, ProviderUsagePoller,
    SessionGeneration,
};
use tokenmon_provider_opencode::budget::ScanFn;
use tokenmon_provider_opencode::captions;
use tokenmon_provider_opencode::db::{DbError, MessageRow, Scan};
use tokenmon_provider_opencode::poller::{
    FetchConsoleFn, OpenCodePollerDeps, OpenCodeSession, OpenCodeUsagePoller,
};
use tokenmon_provider_opencode::pricing::Tokens;
use tokenmon_provider_opencode::snapshot::{OpenCodeSnapshot, WindowKind, WindowUsage};

const CTX: ProviderErrorContext = ProviderErrorContext::OpenCode;

// ---------------------------------------------------------------- fixtures

#[derive(Default)]
struct SessionInner {
    state: AuthSessionState,
    cookie: Option<String>,
    workspace: Option<String>,
    listeners: Vec<Arc<dyn AuthEventListener>>,
}

/// In-memory OpenCode session: the real pure state machine plus sign-out
/// events (sign-out also forgets the workspace, like `OpenCodeAuthSession`).
#[derive(Default)]
struct FakeSession {
    inner: Mutex<SessionInner>,
}

impl FakeSession {
    fn signed_in(cookie: &str) -> Arc<Self> {
        let s = Arc::new(Self::default());
        s.save(cookie);
        s
    }

    fn signed_out() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn save(&self, cookie: &str) {
        let mut g = self.inner.lock().unwrap();
        g.cookie = Some(cookie.to_owned());
        g.state.mark_signed_in(None);
    }

    fn sign_out(&self) {
        let listeners = {
            let mut g = self.inner.lock().unwrap();
            g.cookie = None;
            g.workspace = None;
            g.state.mark_session_invalid(None);
            g.listeners.clone()
        };
        for l in listeners {
            l.on_auth_event(AuthEvent::SignedOut);
        }
    }

    fn workspace(&self) -> Option<String> {
        self.inner.lock().unwrap().workspace.clone()
    }

    fn snapshot(&self) -> AuthSessionState {
        self.inner.lock().unwrap().state.clone()
    }
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        self.inner.lock().unwrap().cookie.clone()
    }
    fn state(&self) -> AuthSessionState {
        self.snapshot()
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        let invalid = g.state.record_auth_failure(reason);
        if invalid {
            g.cookie = None;
        }
        invalid
    }
    fn record_auth_success(&self) {
        self.inner.lock().unwrap().state.record_auth_success();
    }
    fn is_current(&self, generation: SessionGeneration) -> bool {
        self.inner.lock().unwrap().state.is_current(generation)
    }
    fn add_listener(&self, listener: Arc<dyn AuthEventListener>) {
        self.inner.lock().unwrap().listeners.push(listener);
    }
}

impl OpenCodeSession for FakeSession {
    fn workspace_id(&self) -> Option<String> {
        self.workspace()
    }
    fn set_workspace_id(&self, id: &str) {
        self.inner.lock().unwrap().workspace = Some(id.to_owned());
    }
    fn set_needs_sign_in(&self, needs: bool) {
        self.inner.lock().unwrap().state.needs_sign_in = needs;
    }
}

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap()
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(t0()))
}

fn go_row(hours_ago: i64, cost: f64) -> MessageRow {
    MessageRow {
        date: t0() - chrono::Duration::hours(hours_ago),
        session_id: "s1".into(),
        provider_id: "opencode-go".into(),
        model_id: "kimi-k2.5".into(),
        cost_usd: cost,
        tokens: Tokens {
            input: 100,
            output: 10,
            cache_read: 0,
            cache_write: 0,
        },
    }
}

/// A scan with one Go message $2 an hour ago and a Go subscription since Aug 1.
fn local_scan() -> Scan {
    Scan {
        subscribed_at: Some(Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap()),
        rows: vec![go_row(1, 2.0)],
    }
}

fn scan_seam(scan: Scan) -> Arc<ScanFn> {
    let scan = Arc::new(scan);
    Arc::new(move |_since| Ok(Arc::clone(&scan)))
}

fn unreadable_seam() -> Arc<ScanFn> {
    Arc::new(|_| Err(DbError::Missing("opencode.db".into())))
}

fn console_snapshot(monthly_used: f64) -> OpenCodeSnapshot {
    OpenCodeSnapshot {
        windows: vec![WindowUsage {
            kind: WindowKind::Monthly,
            used_usd: monthly_used,
            limit_usd: 60.0,
            resets_at: Some(Utc.with_ymd_and_hms(2026, 9, 5, 0, 0, 0).unwrap()),
            session_count: 0,
        }],
        ..OpenCodeSnapshot::default()
    }
}

type Outcome = Result<(OpenCodeSnapshot, String), ProviderError>;

fn console(outcome: Outcome) -> Arc<FetchConsoleFn> {
    Arc::new(move |_cookie, _org| {
        let outcome = outcome.clone();
        Box::pin(async move { outcome })
    })
}

fn unauthorized() -> Arc<FetchConsoleFn> {
    console(Err(ProviderError::Unauthorized(CTX)))
}

struct Rig {
    poller: Arc<OpenCodeUsagePoller>,
    auth: Arc<FakeSession>,
}

fn rig_with(
    auth: Arc<FakeSession>,
    fetch_console: Arc<FetchConsoleFn>,
    load_scan: Arc<ScanFn>,
) -> Rig {
    let settings = Arc::new(AppSettings::new(Arc::new(SettingsFile::in_memory())));
    settings.set_selected_provider(ProviderId::OpenCode);
    let poller = OpenCodeUsagePoller::new(OpenCodePollerDeps {
        settings,
        auth: auth.clone(),
        clock: clock(),
        fetch_console,
        load_scan,
    });
    Rig { poller, auth }
}

// ------------------------------------------------------------- happy paths

/// A good console fetch publishes the console windows merged with the local
/// models and monthly totals, remembers the workspace and clears the prompt.
#[tokio::test]
async fn test_console_success_publishes_merged_snapshot() {
    let auth = FakeSession::signed_in("auth=1");
    auth.inner.lock().unwrap().state.needs_sign_in = false;
    let rig = rig_with(
        auth,
        console(Ok((console_snapshot(30.0), "wrk_1".into()))),
        scan_seam(local_scan()),
    );

    rig.poller.refresh_now().await;

    let snapshot = rig.poller.snapshot().unwrap();
    assert!(!snapshot.is_estimated);
    assert!((snapshot.monthly_used_percent() - 50.0).abs() < 1e-9);
    assert!(!snapshot.models.is_empty());
    assert_eq!(
        rig.poller.data_source_label(),
        Some(captions::SOURCE_CONSOLE)
    );
    assert_eq!(rig.poller.last_error_text(), None);
    assert_eq!(rig.auth.workspace().as_deref(), Some("wrk_1"));
    assert!(rig.poller.last_refreshed_at().is_some());
    assert!(rig.poller.day_hourly_usage().is_some());
    assert_eq!(rig.poller.headline_used_percent(), Some(50.0));
    let days = rig.poller.daily_budget_days().unwrap();
    assert_eq!(days.len(), 7);
    assert!(
        rig.poller
            .daily_budget_days_at(WeekOffset::new(-1))
            .is_some()
    );
}

/// Without a cookie the local estimate is published with the sign-in caption.
#[tokio::test]
async fn test_local_estimate_publishes_when_started_signed_out() {
    let rig = rig_with(
        FakeSession::signed_out(),
        unauthorized(),
        scan_seam(local_scan()),
    );

    rig.poller.refresh_now().await;

    let snapshot = rig.poller.snapshot().unwrap();
    assert!(snapshot.is_estimated);
    assert_eq!(rig.poller.data_source_label(), Some(captions::SOURCE_LOCAL));
    assert_eq!(
        rig.poller.last_error_text().as_deref(),
        Some(captions::NO_COOKIE)
    );
    assert!(rig.poller.daily_budget_days().is_some());
}

/// A sign-out while the local scan runs suppresses the estimate.
#[tokio::test]
async fn test_local_estimate_suppressed_after_mid_poll_sign_out() {
    let auth = FakeSession::signed_out();
    let signer = auth.clone();
    let done = Arc::new(AtomicBool::new(false));
    let scan = Arc::new(local_scan());
    let seam: Arc<ScanFn> = Arc::new(move |_since| {
        if !done.swap(true, Ordering::SeqCst) {
            signer.sign_out();
        }
        Ok(Arc::clone(&scan))
    });
    let rig = rig_with(auth, unauthorized(), seam);

    rig.poller.refresh_now().await;

    assert!(rig.poller.snapshot().is_none());
}

/// Local database unreadable: the dedicated caption, no snapshot.
#[tokio::test]
async fn test_unreadable_local_database_reports_caption() {
    let rig = rig_with(FakeSession::signed_out(), unauthorized(), unreadable_seam());

    rig.poller.refresh_now().await;

    assert!(rig.poller.snapshot().is_none());
    assert_eq!(
        rig.poller.last_error_text().as_deref(),
        Some(captions::LOCAL_UNREADABLE)
    );
    assert!(matches!(
        rig.poller.last_error(),
        Some(ProviderError::BadResponse(CTX, _))
    ));
}

/// Sign-out drops the snapshot, bars, label and error.
#[tokio::test]
async fn test_sign_out_clears_everything() {
    let rig = rig_with(
        FakeSession::signed_in("auth=1"),
        console(Ok((console_snapshot(30.0), "wrk_1".into()))),
        scan_seam(local_scan()),
    );
    rig.poller.refresh_now().await;
    assert!(rig.poller.snapshot().is_some());

    rig.auth.sign_out();

    assert!(rig.poller.snapshot().is_none());
    assert!(rig.poller.daily_budget_days().is_none());
    assert!(rig.poller.data_source_label().is_none());
    assert!(rig.poller.day_hourly_usage().is_none());
}

/// A disabled provider is not refreshed.
#[tokio::test]
async fn test_disabled_provider_is_not_refreshed() {
    let settings = Arc::new(AppSettings::new(Arc::new(SettingsFile::in_memory())));
    let enabled = [ProviderId::Grok].into_iter().collect();
    settings.set_enabled_providers(&enabled).unwrap();
    let calls = Arc::new(AtomicU32::new(0));
    let counter = calls.clone();
    let fetch: Arc<FetchConsoleFn> = Arc::new(move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(ProviderError::Unauthorized(CTX)) })
    });
    let poller = OpenCodeUsagePoller::new(OpenCodePollerDeps {
        settings,
        auth: FakeSession::signed_in("auth=1"),
        clock: clock(),
        fetch_console: fetch,
        load_scan: scan_seam(local_scan()),
    });

    poller.refresh_now().await;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(poller.snapshot().is_none());
}

// ------------------------------------------------- session guard (PollerSession)

/// A console fetch that signs the session out and back in before returning.
fn swapping_fetch(auth: &Arc<FakeSession>, result: Outcome) -> Arc<FetchConsoleFn> {
    let auth = auth.clone();
    Arc::new(move |_, _| {
        let auth = auth.clone();
        let result = result.clone();
        Box::pin(async move {
            auth.sign_out();
            auth.save("auth=new");
            result
        })
    })
}

/// A success that finishes after the session was replaced is dropped.
#[tokio::test]
async fn test_open_code_stale_success_is_dropped() {
    let auth = FakeSession::signed_in("auth=old");
    let fetch = swapping_fetch(&auth, Ok((console_snapshot(30.0), "wrk_1".into())));
    let rig = rig_with(auth, fetch, scan_seam(local_scan()));

    rig.poller.refresh_now().await;

    assert!(rig.poller.snapshot().is_none());
}

/// A late success must not restore the workspace the sign-out cleared.
#[tokio::test]
async fn test_open_code_late_success_does_not_restore_workspace_id() {
    let auth = FakeSession::signed_in("auth=old");
    let fetch = swapping_fetch(&auth, Ok((console_snapshot(30.0), "wrk_late".into())));
    let rig = rig_with(auth, fetch, scan_seam(local_scan()));

    rig.poller.refresh_now().await;

    assert_eq!(rig.auth.workspace(), None);
}

/// A 401 that finishes after the session was replaced must not invalidate
/// the new session.
#[tokio::test]
async fn test_open_code_stale_unauthorized_does_not_invalidate_new_session() {
    let auth = FakeSession::signed_in("auth=old");
    let fetch = swapping_fetch(&auth, Err(ProviderError::Unauthorized(CTX)));
    let rig = rig_with(auth.clone(), fetch, scan_seam(local_scan()));

    rig.poller.refresh_now().await;

    let state = auth.state();
    assert!(state.is_signed_in);
    assert!(!state.needs_sign_in);
    assert_eq!(state.consecutive_auth_failures, 0);
    assert!(rig.poller.snapshot().is_none());
}

/// Repeated 401s on the live session invalidate it after the threshold.
#[tokio::test]
async fn test_open_code_current_unauthorized_invalidates_session() {
    let rig = rig_with(
        FakeSession::signed_in("auth=1"),
        unauthorized(),
        scan_seam(local_scan()),
    );
    for _ in 0..AUTH_FAILURE_THRESHOLD {
        rig.poller.refresh_now().await;
    }
    let state = rig.auth.state();
    assert!(state.needs_sign_in);
    assert!(!state.is_signed_in);
}

// ---------------------------------------------------------- fallback captions

/// The console always answers 401: the first polls show the generic caption,
/// the poll that invalidates the session still publishes the estimate with
/// the session-expired caption.
#[tokio::test]
async fn test_expired_console_session_falls_back_to_local_estimate() {
    let rig = rig_with(
        FakeSession::signed_in("auth=1"),
        unauthorized(),
        scan_seam(local_scan()),
    );

    rig.poller.refresh_now().await;
    assert!(!rig.auth.state().needs_sign_in);
    assert!(rig.poller.snapshot().unwrap().is_estimated);
    assert_eq!(
        rig.poller.last_error_text().as_deref(),
        Some("Console fetch failed \u{2014} showing local estimate.")
    );

    for _ in 1..AUTH_FAILURE_THRESHOLD {
        rig.poller.refresh_now().await;
    }
    assert!(rig.auth.state().needs_sign_in);
    assert!(rig.poller.snapshot().unwrap().is_estimated);
    assert_eq!(rig.poller.data_source_label(), Some("Local estimate"));
    assert_eq!(
        rig.poller.last_error_text().as_deref(),
        Some(
            "Console session expired \u{2014} showing local estimate. Sign in again for official numbers."
        )
    );
}

/// A non-auth console failure (network) keeps the generic caption and asks
/// the loop to back off.
#[tokio::test]
async fn test_network_failure_falls_back_and_reports_failure() {
    let rig = rig_with(
        FakeSession::signed_in("auth=1"),
        console(Err(ProviderError::Network(CTX, "reset".into()))),
        scan_seam(local_scan()),
    );

    rig.poller.refresh_now().await;

    assert_eq!(rig.poller.data_source_label(), Some(captions::SOURCE_LOCAL));
    assert_eq!(
        rig.poller.last_error_text().as_deref(),
        Some(captions::FETCH_FAILED)
    );
    assert!(!rig.auth.state().needs_sign_in);
}
