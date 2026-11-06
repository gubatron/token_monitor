//! Session-sharing behaviour (ports of `PollerSessionGuardTests` and
//! `AuthInvalidationTests` for Grokbot). The session is a Cursor session.

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::sync::{Arc, Mutex};

use tokenmon_core::{
    AuthEventListener, AuthSession, AuthSessionState, FixedClock, ProviderError,
    ProviderErrorContext, SessionGeneration, UsageFetcher,
};
use tokenmon_provider_cursor::CursorHttp;
use tokenmon_provider_grokbot::{GrokbotFetcher, GrokbotUsageClient};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A fake Cursor session over the real state machine.
struct FakeSession {
    state: Mutex<AuthSessionState>,
}

impl FakeSession {
    fn signed_in() -> Arc<Self> {
        let mut state = AuthSessionState::default();
        state.mark_signed_in(Some("user@example.com".into()));
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut AuthSessionState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }
}

impl AuthSession for FakeSession {
    fn cookie_header(&self) -> Option<String> {
        self.with(|s| s.is_signed_in)
            .then(|| "WorkosCursorSessionToken=x".to_owned())
    }
    fn state(&self) -> AuthSessionState {
        self.with(|s| s.clone())
    }
    fn record_auth_failure(&self, reason: &str) -> bool {
        self.with(|s| s.record_auth_failure(reason))
    }
    fn record_auth_success(&self) {
        self.with(AuthSessionState::record_auth_success);
    }
    fn is_current(&self, generation: SessionGeneration) -> bool {
        self.with(|s| s.is_current(generation))
    }
    fn add_listener(&self, _listener: Arc<dyn AuthEventListener>) {}
}

/// A `CursorHttp` for `base_url` with Grokbot wording and a fixed clock.
fn cursor_http(base_url: &str) -> CursorHttp {
    CursorHttp::with_base_url(
        ProviderErrorContext::Grokbot,
        base_url,
        Some("test"),
        Arc::new(FixedClock::from_unix(1_787_000_000)),
    )
    .unwrap()
}

fn fetcher(server: &MockServer, session: &Arc<FakeSession>) -> GrokbotFetcher {
    GrokbotFetcher::new(
        GrokbotUsageClient::new(cursor_http(&server.uri())),
        Arc::clone(session) as Arc<dyn AuthSession>,
        Arc::new(FixedClock::from_unix(1_787_000_000)),
    )
}

async fn mount(server: &MockServer, template: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/api/dashboard/get-sand-usage-status"))
        .respond_with(template)
        .mount(server)
        .await;
}

/// A 401 for a session that was replaced during the request must not strike
/// the new session.
#[tokio::test]
async fn test_stale_unauthorized_does_not_invalidate_new_session() {
    let server = MockServer::start().await;
    let session = FakeSession::signed_in();
    let swapper = Arc::clone(&session);
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            // Sign-out and sign-in while the request is in flight.
            swapper.with(|s| s.mark_signed_in(Some("user@example.com".into())));
            ResponseTemplate::new(401)
        })
        .mount(&server)
        .await;
    let result = fetcher(&server, &session).fetch().await;
    assert_eq!(
        result.unwrap_err(),
        ProviderError::Unauthorized(ProviderErrorContext::Grokbot)
    );
    let state = session.state();
    assert!(state.is_signed_in && !state.needs_sign_in);
    assert_eq!(state.consecutive_auth_failures, 0);
}

/// Three 401s on the current session invalidate the Cursor session.
#[tokio::test]
async fn test_current_unauthorized_invalidates_session_on_third_strike() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(401)).await;
    let session = FakeSession::signed_in();
    let f = fetcher(&server, &session);
    for _ in 0..2 {
        f.fetch().await.unwrap_err();
        assert!(session.state().is_signed_in);
    }
    assert_eq!(session.state().consecutive_auth_failures, 2);
    f.fetch().await.unwrap_err();
    let state = session.state();
    assert!(state.needs_sign_in && !state.is_signed_in);
    assert_eq!(
        state.last_auth_error.as_deref(),
        Some("Cursor rejected the session")
    );
}

/// Four non-challenge 403s: counter stays 0, session intact, error message is
/// the "not available" text.
#[tokio::test]
async fn test_forbidden_does_not_invalidate_cursor_session() {
    let server = MockServer::start().await;
    mount(&server, ResponseTemplate::new(403).set_body_string("{}")).await;
    let session = FakeSession::signed_in();
    let f = fetcher(&server, &session);
    for _ in 0..4 {
        let e = f.fetch().await.unwrap_err();
        assert_eq!(
            e.to_string(),
            "Grok Bot is not available on this Cursor account."
        );
    }
    let state = session.state();
    assert_eq!(state.consecutive_auth_failures, 0);
    assert!(state.is_signed_in && !state.needs_sign_in);
}

/// A success resets the strike counter.
#[tokio::test]
async fn test_success_resets_strikes() {
    let server = MockServer::start().await;
    mount(
        &server,
        ResponseTemplate::new(200).set_body_string(r#"{"usagePercent":4}"#),
    )
    .await;
    let session = FakeSession::signed_in();
    session.record_auth_failure("x");
    let f = fetcher(&server, &session);
    let snapshot = f.fetch().await.unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(session.state().consecutive_auth_failures, 0);
    assert_eq!(f.snapshot(), Some(snapshot));
}

/// Without a cookie the fetch fails with the Grokbot sign-in message and makes
/// no request.
#[tokio::test]
async fn test_signed_out_fetch_is_not_signed_in() {
    let server = MockServer::start().await;
    let session = Arc::new(FakeSession {
        state: Mutex::new(AuthSessionState::default()),
    });
    let e = fetcher(&server, &session).fetch().await.unwrap_err();
    assert_eq!(
        e.to_string(),
        "Sign in to Cursor to load your Grokbot allowance."
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}
