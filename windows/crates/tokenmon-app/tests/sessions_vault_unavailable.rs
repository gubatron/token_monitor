//! Decision #38: when the vault cannot take a sign-in, the sign-in window and
//! the Accounts row say so clearly, and nothing is written unencrypted.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokenmon_app::sessions::{HostBrowserCleaner, Sessions, friendly_store_error};
use tokenmon_app::signin::SignInService;
use tokenmon_core::{
    BrowserStateCleaner, Clock, CredentialStoreError, ProviderId, SystemClock,
    VAULT_UNAVAILABLE_MESSAGE,
};
use tokenmon_platform::secrets::{FakeEncryptor, VAULT_UNAVAILABLE_REASON, Vault};
use tokenmon_platform::webauth::{FakeWebAuthHost, Input, ViewKind, WebAuthHost, WebCookie};

const COOKIE_VALUE: &str = "VERY-SECRET-PLAINTEXT-MARKER";

/// Message the user sees; also pinned verbatim so a rewording is deliberate.
const EXPECTED: &str = "Couldn't save your sign-in securely. TokenMon never stores credentials unencrypted. Check that your Windows account can use Data Protection, then try again.";

#[test]
fn the_message_matches_the_maintainer_wording() {
    assert_eq!(VAULT_UNAVAILABLE_MESSAGE, EXPECTED);
}

#[test]
fn raw_vault_errors_are_rewritten_and_others_are_kept() {
    let raw = CredentialStoreError::WriteFailed(VAULT_UNAVAILABLE_REASON.into()).to_string();
    assert_eq!(friendly_store_error(&raw), EXPECTED);
    assert_eq!(friendly_store_error("disk full"), "disk full");
}

#[test]
fn a_sign_in_the_vault_cannot_save_shows_the_message_and_stays_signed_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    let enc = Arc::new(FakeEncryptor::default());
    enc.set_fail_protect(true);
    let host = Arc::new(FakeWebAuthHost::default());
    let vault = Arc::new(Vault::new(dir.path(), enc));
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

    service.start(ProviderId::Grok).expect("opens");
    let window = host
        .sessions
        .lock()
        .expect("lock")
        .get("grok")
        .cloned()
        .expect("window");
    {
        let mut w = window.lock().expect("lock");
        w.set_cookies(vec![WebCookie::new("sso", COOKIE_VALUE, ".grok.com")]);
        for url in ["https://accounts.x.ai/sign-in", "https://grok.com/"] {
            w.send(Input::NavigationFinished {
                view: ViewKind::Main,
                url: url.to_owned(),
            });
        }
        w.advance(Duration::from_secs(5));
        // The sign-in window itself carries the message.
        assert!(
            w.machine().view_model().status_text.contains(EXPECTED),
            "window said: {}",
            w.machine().view_model().status_text
        );
    }
    // The Accounts row / panel read it from the session state.
    let state = sessions.auth_state(ProviderId::Grok);
    assert!(!state.is_signed_in);
    assert_eq!(state.last_auth_error.as_deref(), Some(EXPECTED));
    assert!(completed.lock().expect("lock").is_empty());
    // Nothing unencrypted anywhere in the data directory.
    for entry in std::fs::read_dir(dir.path()).expect("dir") {
        let bytes = std::fs::read(entry.expect("entry").path()).expect("read");
        assert!(
            !String::from_utf8_lossy(&bytes).contains(COOKIE_VALUE),
            "secret reached the disk"
        );
    }
}
