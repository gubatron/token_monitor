//! Sessions over the vault and sign-in service wiring, against platform fakes.

use std::sync::{Arc, Mutex};

use tokenmon_app::sessions::{HostBrowserCleaner, Sessions, slug};
use tokenmon_app::signin::{SignInService, sign_in_config};
use tokenmon_core::{
    AuthSession, BrowserStateCleaner, Clock, ProviderAuthSession, ProviderId, SystemClock,
};
use tokenmon_platform::secrets::{FakeEncryptor, Vault};
use tokenmon_platform::webauth::{FakeWebAuthHost, WebAuthHost};

struct Rig {
    _dir: tempfile::TempDir,
    host: Arc<FakeWebAuthHost>,
    sessions: Arc<Sessions>,
    completed: Arc<Mutex<Vec<ProviderId>>>,
    service: SignInService,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = Arc::new(FakeWebAuthHost::default());
    let vault = Arc::new(Vault::new(dir.path(), Arc::new(FakeEncryptor::default())));
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let cleaner: Arc<dyn BrowserStateCleaner> = Arc::new(HostBrowserCleaner::new(
        Arc::clone(&host) as Arc<dyn WebAuthHost>,
    ));
    let sessions = Arc::new(Sessions::open(dir.path(), &vault, &clock, &cleaner));
    let completed = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&completed);
    let service = SignInService::new(
        Arc::clone(&host) as Arc<dyn WebAuthHost>,
        Arc::clone(&sessions),
        Arc::new(move |p| sink.lock().expect("lock").push(p)),
    );
    Rig {
        _dir: dir,
        host,
        sessions,
        completed,
        service,
    }
}

/// Grokbot signs in through Cursor's window and shares Cursor's session.
#[test]
fn test_grokbot_shares_cursor_session_and_window() {
    let r = rig();
    assert_eq!(
        r.sessions.key(ProviderId::Grokbot),
        r.sessions.key(ProviderId::Cursor)
    );
    assert_ne!(
        r.sessions.key(ProviderId::Claude),
        r.sessions.key(ProviderId::Cursor)
    );
    assert_eq!(slug(ProviderId::Grokbot), "cursor");
    r.service.start(ProviderId::Grokbot).expect("opens");
    assert_eq!(*r.host.opened.lock().expect("lock"), vec!["cursor"]);
}

/// Every cookie provider has a window; OpenRouter (API key) does not.
#[test]
fn test_every_cookie_provider_has_a_sign_in_config() {
    for p in [
        ProviderId::Grok,
        ProviderId::Cursor,
        ProviderId::Claude,
        ProviderId::ChatGpt,
        ProviderId::OpenCode,
        ProviderId::Grokbot,
    ] {
        let c = sign_in_config(p).unwrap_or_else(|| panic!("{p} has a config"));
        assert_eq!(c.provider_slug, slug(p));
        assert!(c.start_url.starts_with("https://"));
    }
    assert!(sign_in_config(ProviderId::OpenRouter).is_none());
    assert!(r_start_fails());
}

fn r_start_fails() -> bool {
    rig().service.start(ProviderId::OpenRouter).is_err()
}

/// Grok accepts both `sso` and `sso-rw` as session cookies.
#[test]
fn test_grok_accepts_sso_and_sso_rw() {
    let c = sign_in_config(ProviderId::Grok).expect("config");
    assert!(tokenmon_provider_grok::auth::is_preferred_session_cookie(
        "sso"
    ));
    assert!(tokenmon_provider_grok::auth::is_preferred_session_cookie(
        "sso-rw"
    ));
    for name in ["sso", "sso-rw"] {
        assert!(
            c.policy
                .auth
                .essential_cookie_names
                .iter()
                .any(|n| n == name)
        );
    }
    assert!((c.policy.is_domain)("accounts.grok.com") || (c.policy.is_domain)("grok.com"));
}

/// Signing out clears the credentials, then the embedded browser profile.
#[test]
fn test_sign_out_clears_session_and_browser_profile() {
    let r = rig();
    r.sessions.claude.save("sessionKey=abc").expect("saved");
    assert!(r.sessions.claude.state().is_signed_in);
    r.service.sign_out(ProviderId::Claude);
    assert!(!r.sessions.claude.state().is_signed_in);
    assert!(
        r.host
            .signed_out
            .lock()
            .expect("lock")
            .contains(&"claude".to_owned())
    );
    assert!(r.completed.lock().expect("lock").is_empty());
}

/// Credentials survive reopening the sessions over the same vault.
#[test]
fn test_session_persists_in_the_vault() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = Arc::new(Vault::new(dir.path(), Arc::new(FakeEncryptor::default())));
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let host: Arc<dyn WebAuthHost> = Arc::new(FakeWebAuthHost::default());
    let cleaner: Arc<dyn BrowserStateCleaner> = Arc::new(HostBrowserCleaner::new(host));
    let open = || Sessions::open(dir.path(), &vault, &clock, &cleaner);
    open().grok.save("sso=1; sso-rw=2").expect("saved");
    assert!(open().grok.state().is_signed_in);
}

/// A full Grok sign-in through the headless window: landing on grok.com with
/// an `sso` cookie adopts the session, signs it in and reports completion.
#[test]
fn test_grok_sign_in_adopts_the_captured_session_and_completes() {
    use std::time::Duration;
    use tokenmon_platform::webauth::{Input, ViewKind, WebCookie};

    let r = rig();
    r.service.start(ProviderId::Grok).expect("opens");
    let headless = r
        .host
        .sessions
        .lock()
        .expect("lock")
        .get("grok")
        .cloned()
        .expect("window");
    {
        let mut w = headless.lock().expect("lock");
        w.set_cookies(vec![WebCookie::new("sso", "token", ".grok.com")]);
        for url in ["https://accounts.x.ai/sign-in", "https://grok.com/"] {
            w.send(Input::NavigationFinished {
                view: ViewKind::Main,
                url: url.to_owned(),
            });
        }
        w.advance(Duration::from_secs(5));
    }
    assert!(r.sessions.grok.state().is_signed_in);
    assert_eq!(*r.completed.lock().expect("lock"), vec![ProviderId::Grok]);
}

/// Each provider's predicates tell sign-in pages from the signed-in return
/// page.
#[test]
fn test_return_page_predicates_follow_each_provider() {
    let page = |p: ProviderId, url: &str| {
        let c = sign_in_config(p).expect("config");
        (c.is_return_page)(&url::Url::parse(url).expect("url"))
    };
    assert!(page(ProviderId::Grok, "https://grok.com/"));
    assert!(!page(ProviderId::Grok, "https://accounts.x.ai/sign-in"));
    assert!(page(ProviderId::Claude, "https://claude.ai/new"));
    assert!(!page(ProviderId::Claude, "https://claude.ai/login"));
    assert!(page(ProviderId::ChatGpt, "https://chatgpt.com/"));
    assert!(page(
        ProviderId::Cursor,
        "https://cursor.com/dashboard/usage"
    ));
    assert!(page(
        ProviderId::OpenCode,
        "https://opencode.ai/console/wrk_1"
    ));
    assert!(!page(
        ProviderId::OpenCode,
        "https://opencode.ai/console/login"
    ));
}

/// A host whose browser cannot start (no WebView2).
struct NoBrowser;

impl WebAuthHost for NoBrowser {
    fn open(
        &self,
        _config: tokenmon_platform::webauth::SignInConfig,
        _delegate: Arc<dyn tokenmon_platform::webauth::SignInDelegate>,
    ) -> Result<(), tokenmon_platform::webauth::WebAuthError> {
        Err(
            tokenmon_platform::webauth::WebAuthError::BrowserUnavailable(
                "runtime not found".into(),
            ),
        )
    }
    fn sign_out(&self, _slug: &str) {}
}

/// A sign-in window that cannot open leaves a visible error with the
/// WebView2 download link (https only), shared by Grokbot and Cursor, and a
/// sign-out clears it (#70).
#[test]
fn test_sign_in_that_cannot_open_surfaces_a_webview2_error() {
    use tokenmon_app::signin::WEBVIEW2_DOWNLOAD_URL;
    let r = rig();
    let service = SignInService::new(
        Arc::new(NoBrowser),
        Arc::clone(&r.sessions),
        Arc::new(|_| {}),
    );
    assert!(r.sessions.sign_in_error(ProviderId::Cursor).is_none());
    assert!(service.start(ProviderId::Grokbot).is_err());
    let message = r
        .sessions
        .sign_in_error(ProviderId::Cursor)
        .expect("error recorded under Cursor's session");
    assert!(message.contains("WebView2") && message.contains(WEBVIEW2_DOWNLOAD_URL));
    assert!(WEBVIEW2_DOWNLOAD_URL.starts_with("https://"));
    assert!(r.sessions.sign_in_error(ProviderId::Grokbot).is_some());
    service.sign_out(ProviderId::Cursor);
    assert!(r.sessions.sign_in_error(ProviderId::Cursor).is_none());
}

/// A later successful open clears the stale error.
#[test]
fn test_successful_open_clears_the_error() {
    let r = rig();
    r.sessions
        .set_sign_in_error(ProviderId::Claude, Some("old".into()));
    r.service.start(ProviderId::Claude).expect("opens");
    assert!(r.sessions.sign_in_error(ProviderId::Claude).is_none());
}
