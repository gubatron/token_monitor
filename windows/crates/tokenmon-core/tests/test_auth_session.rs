//! `CookieAuthSession` against `InMemoryCredentialStore` (spec 01 1.6.4):
//! ports of ProviderAuthSessionTests and CredentialStoreTests.testInjectedStoreBacksSession.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use tokenmon_core::{
    AUTH_FAILURE_THRESHOLD, AuthConfig, AuthEvent, AuthEventListener, AuthSession,
    BrowserStateCleaner, CapturedSession, CookieAuthSession, CredentialKey, CredentialStore,
    InMemoryCredentialStore, ManualClock, ProviderAuthSession,
};

fn config(essential: &[&str], prefixes: &[&str], identity: Option<&str>) -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "auth_".into(),
        log_category: "TestAuth".into(),
        hosts: vec![],
        sign_out_hosts: vec!["example.com".into()],
        extra_store_keys: vec![CredentialKey::Workspace],
        preferred_session_cookie: "session".into(),
        essential_cookie_names: essential.iter().map(|s| (*s).into()).collect(),
        essential_cookie_prefixes: prefixes.iter().map(|s| (*s).into()).collect(),
        account_identity_cookie: identity.map(Into::into),
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "no cookie".into(),
    }
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::from_unix(1_800_000_000))
}

fn session_with(cfg: AuthConfig) -> (Arc<InMemoryCredentialStore>, CookieAuthSession) {
    let store = Arc::new(InMemoryCredentialStore::default());
    let s = CookieAuthSession::new(cfg, store.clone(), clock());
    (store, s)
}

fn session() -> (Arc<InMemoryCredentialStore>, CookieAuthSession) {
    session_with(config(&[], &[], None))
}

#[derive(Default)]
struct Events(Mutex<Vec<AuthEvent>>);
impl AuthEventListener for Events {
    fn on_auth_event(&self, e: AuthEvent) {
        self.0.lock().unwrap().push(e);
    }
}
impl Events {
    fn count(&self, e: AuthEvent) -> usize {
        self.0.lock().unwrap().iter().filter(|x| **x == e).count()
    }
}

fn observe(s: &CookieAuthSession) -> Arc<Events> {
    let ev = Arc::new(Events::default());
    s.add_listener(ev.clone());
    ev
}

/// Starts signed out and needing sign-in.
#[test]
fn test_starts_with_signed_out_state() {
    let (_, s) = session();
    let st = s.state();
    assert!(!st.is_signed_in && st.needs_sign_in && st.account_email.is_none());
}

/// The email persists and a fresh session over the same store reads it.
#[test]
fn test_save_and_load_account_email_persists() {
    let (store, s) = session();
    s.save_account_email("user@example.com");
    assert_eq!(s.state().account_email.as_deref(), Some("user@example.com"));
    let again = CookieAuthSession::new(config(&[], &[], None), store, clock());
    assert_eq!(
        again.state().account_email.as_deref(),
        Some("user@example.com")
    );
}

/// Invalidation clears flags, the email and the stored secrets.
#[test]
fn test_mark_session_invalid_clears_state() {
    let (store, s) = session();
    s.save("sid=1").unwrap();
    s.save_account_email("user@example.com");
    store.set(CredentialKey::Workspace, "w").unwrap();
    s.mark_session_invalid(Some("401"));
    let st = s.state();
    assert!(!st.is_signed_in && st.needs_sign_in);
    assert_eq!(st.last_auth_error.as_deref(), Some("401"));
    assert_eq!(st.account_email, None);
    assert_eq!(store.get(CredentialKey::Session), None);
    assert_eq!(store.get(CredentialKey::Email), None);
    assert_eq!(store.get(CredentialKey::Workspace), None);
}

/// A generation captured before sign-out stays stale after re-auth.
#[test]
fn test_generation_invalidates_in_flight_refresh() {
    let (_, s) = session();
    s.save("session=abc").unwrap();
    let g = s.session_generation();
    assert!(s.is_current(g));
    s.sign_out();
    assert!(!s.is_current(g));
    s.save("session=def").unwrap();
    assert!(s.is_current(s.session_generation()));
    assert!(!s.is_current(g));
}

/// `is_current` needs a usable session even when the generation is unchanged.
#[test]
fn test_is_current_requires_usable_session() {
    let (_, s) = session();
    s.save("session=abc").unwrap();
    s.set_needs_sign_in(true);
    assert!(!s.is_current(s.session_generation()));
}

/// Non-essential cookies are dropped on load and the narrowed jar persists.
#[test]
fn test_load_narrows_stored_cookie_jar_to_allowlist() {
    let (store, s) = session_with(config(&["sso", "sso-rw"], &[], None));
    s.save("auth_token=x; ct0=y; sso=z; sso-rw=w; twid=t")
        .unwrap();
    assert_eq!(s.cookie_header().as_deref(), Some("sso=z; sso-rw=w"));
    assert_eq!(
        store.get(CredentialKey::Session).as_deref(),
        Some("sso=z; sso-rw=w")
    );
}

/// A jar without any essential cookie is left alone.
#[test]
fn test_load_keeps_jar_when_essentials_absent() {
    let (_, s) = session_with(config(&["sso"], &[], None));
    s.save("auth_token=x; ct0=y").unwrap();
    assert_eq!(s.cookie_header().as_deref(), Some("auth_token=x; ct0=y"));
}

/// A chunked essential family is kept whole.
#[test]
fn test_load_keeps_chunked_essential_family() {
    let (_, s) = session_with(config(&[], &["__secure-next-auth.session-token"], None));
    s.save("__Host-next-auth.csrf-token=c; __Secure-next-auth.session-token.0=a; __Secure-next-auth.session-token.1=b; _ga=g").unwrap();
    assert_eq!(
        s.cookie_header().as_deref(),
        Some("__Secure-next-auth.session-token.0=a; __Secure-next-auth.session-token.1=b")
    );
}

/// Only a non-essential cookie present: not narrowed to nothing.
#[test]
fn test_load_leaves_jar_when_only_non_essential_present() {
    let (_, s) = session_with(config(&[], &["__secure-next-auth.session-token"], None));
    s.save("__Host-next-auth.csrf-token=c; _ga=g").unwrap();
    assert_eq!(
        s.cookie_header().as_deref(),
        Some("__Host-next-auth.csrf-token=c; _ga=g")
    );
}

/// Below the threshold the session survives; the third strike invalidates.
#[test]
fn test_session_survives_rejections_below_threshold() {
    let (_, s) = session();
    s.save("session=abc").unwrap();
    for _ in 1..AUTH_FAILURE_THRESHOLD {
        assert!(!s.record_auth_failure("401"));
    }
    assert!(s.state().is_signed_in);
    assert_eq!(s.cookie_header().as_deref(), Some("session=abc"));
    assert!(s.record_auth_failure("401"));
    assert!(!s.state().is_signed_in && s.state().needs_sign_in);
    assert_eq!(s.cookie_header(), None);
}

/// A success between rejections restarts the streak.
#[test]
fn test_auth_success_resets_rejection_streak() {
    let (_, s) = session();
    s.save("session=abc").unwrap();
    for _ in 1..AUTH_FAILURE_THRESHOLD {
        s.record_auth_failure("401");
    }
    s.record_auth_success();
    assert!(!s.record_auth_failure("401"));
    assert!(s.state().is_signed_in);
}

/// Invalidation never fires `AccountReset`.
#[test]
fn test_invalidation_does_not_reset_account_data() {
    let (_, s) = session();
    s.save("session=abc").unwrap();
    let ev = observe(&s);
    s.mark_session_invalid(Some("401"));
    assert_eq!(ev.count(AuthEvent::AccountReset), 0);
}

/// Sign-out fires `SignedOut`; same account again is no reset, another is.
#[test]
fn test_sign_out_keeps_account_data_until_another_account_signs_in() {
    let (_, s) = session();
    s.save("session=abc").unwrap();
    s.save_account_email("one@example.com");
    let ev = observe(&s);
    s.sign_out();
    assert_eq!(ev.count(AuthEvent::SignedOut), 1);
    assert_eq!(ev.count(AuthEvent::AccountReset), 0);
    s.save("session=def").unwrap();
    s.save_account_email("one@example.com");
    assert_eq!(ev.count(AuthEvent::AccountReset), 0);
    s.sign_out();
    s.save("session=ghi").unwrap();
    s.save_account_email("two@example.com");
    assert_eq!(ev.count(AuthEvent::AccountReset), 1);
}

/// Email-identified accounts reset only on a different email.
#[test]
fn test_email_identified_account_change_resets_account_data() {
    let (_, s) = session();
    s.save("session=a").unwrap();
    s.save_account_email("one@example.com");
    let ev = observe(&s);
    s.mark_session_invalid(Some("401"));
    s.save("session=a2").unwrap();
    s.save_account_email("one@example.com");
    assert_eq!(ev.count(AuthEvent::AccountReset), 0);
    s.mark_session_invalid(Some("401"));
    s.save("session=b").unwrap();
    s.save_account_email("two@example.com");
    assert_eq!(ev.count(AuthEvent::AccountReset), 1);
}

fn capture(cookies: &[(&str, &str)], email: Option<&str>) -> CapturedSession {
    CapturedSession {
        cookie_header: cookies
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
        email: email.map(Into::into),
    }
}

/// Cookie-identified accounts (Claude org cookie) reset on a different org.
#[test]
fn test_cookie_identified_account_change_resets_account_data() {
    let (_, s) = session_with(config(&[], &[], Some("lastactiveorg")));
    s.adopt(&capture(
        &[("sessionKey", "k1"), ("lastActiveOrg", "org-1")],
        None,
    ))
    .unwrap();
    let ev = observe(&s);
    s.mark_session_invalid(Some("401"));
    s.adopt(&capture(
        &[("sessionKey", "k2"), ("lastActiveOrg", "org-1")],
        None,
    ))
    .unwrap();
    assert_eq!(ev.count(AuthEvent::AccountReset), 0);
    s.adopt(&capture(
        &[("sessionKey", "k3"), ("lastActiveOrg", "org-2")],
        None,
    ))
    .unwrap();
    assert_eq!(ev.count(AuthEvent::AccountReset), 1);
}

/// A new capture replaces the shown email, even with none.
#[test]
fn test_capture_replaces_previous_account_email() {
    let (store, s) = session();
    s.adopt(&capture(&[("session", "a")], Some("old@example.com")))
        .unwrap();
    assert_eq!(s.state().account_email.as_deref(), Some("old@example.com"));
    s.adopt(&capture(&[("session", "b")], Some("new@example.com")))
        .unwrap();
    assert_eq!(s.state().account_email.as_deref(), Some("new@example.com"));
    s.adopt(&capture(&[("session", "c")], None)).unwrap();
    assert_eq!(s.state().account_email, None);
    assert_eq!(store.get(CredentialKey::Email), None);
}

fn chatgpt(header: &str) -> (Arc<InMemoryCredentialStore>, CookieAuthSession) {
    let (store, s) = session_with(config(&[], &["__secure-next-auth.session-token"], None));
    s.save(header).unwrap();
    (store, s)
}

/// Comma-folded Set-Cookie (with comma-bearing Expires) is fully applied.
#[test]
fn test_refreshed_cookies_parse_comma_folded_header() {
    let (_, s) =
        chatgpt("__Secure-next-auth.session-token.0=old0; __Secure-next-auth.session-token.1=old1");
    let folded = "__Secure-next-auth.session-token.0=new0; Path=/; Expires=Wed, 21 Oct 2099 07:28:00 GMT; Secure; HttpOnly, __Secure-next-auth.session-token.1=new1; Path=/; Expires=Wed, 21 Oct 2099 07:28:00 GMT; Secure; HttpOnly, _ga=tracker; Path=/";
    let generation = s.session_generation();
    s.apply_refreshed_cookies(&[folded.to_owned()]);
    assert_eq!(
        s.cookie_header().as_deref(),
        Some("__Secure-next-auth.session-token.0=new0; __Secure-next-auth.session-token.1=new1")
    );
    assert_eq!(
        s.session_generation(),
        generation,
        "refresh must not bump the generation"
    );
}

/// `Max-Age=0` removes the stored chunk.
#[test]
fn test_refreshed_deletion_cookie_removes_chunk() {
    let (_, s) =
        chatgpt("__Secure-next-auth.session-token.0=a; __Secure-next-auth.session-token.1=b");
    s.apply_refreshed_cookies(&[
        "__Secure-next-auth.session-token.0=whole; Path=/; Secure, __Secure-next-auth.session-token.1=; Path=/; Max-Age=0".into(),
    ]);
    assert_eq!(
        s.cookie_header().as_deref(),
        Some("__Secure-next-auth.session-token.0=whole")
    );
}

/// A renewed unchunked token replaces every stored chunk.
#[test]
fn test_renewed_unchunked_token_replaces_stored_chunks() {
    let (_, s) =
        chatgpt("__Secure-next-auth.session-token.0=a; __Secure-next-auth.session-token.1=b");
    s.apply_refreshed_cookies(&[
        "__Secure-next-auth.session-token=whole; Path=/; Secure; HttpOnly".into(),
    ]);
    assert_eq!(
        s.cookie_header().as_deref(),
        Some("__Secure-next-auth.session-token=whole")
    );
}

/// A past `Expires` is a deletion; the stored value becomes empty.
#[test]
fn test_refreshed_past_expiry_removes_cookie() {
    let (store, s) = chatgpt("__Secure-next-auth.session-token=a");
    s.apply_refreshed_cookies(&[
        "__Secure-next-auth.session-token=a; Expires=Thu, 01 Jan 1970 00:00:00 GMT".into(),
    ]);
    assert_eq!(store.get(CredentialKey::Session).as_deref(), Some(""));
    assert_eq!(s.cookie_header(), None);
}

/// Nothing stored, empty list or no sign-out host: refresh is a no-op.
#[test]
fn test_refreshed_cookies_noop_cases() {
    let (store, s) = session_with(config(&[], &["p"], None));
    s.apply_refreshed_cookies(&["p=1".into()]);
    assert_eq!(store.get(CredentialKey::Session), None);
    s.save("p=0").unwrap();
    s.apply_refreshed_cookies(&[]);
    assert_eq!(store.get(CredentialKey::Session).as_deref(), Some("p=0"));
    let mut cfg = config(&[], &["p"], None);
    cfg.sign_out_hosts.clear();
    let (store2, s2) = session_with(cfg);
    s2.save("p=0").unwrap();
    s2.apply_refreshed_cookies(&["p=1".into()]);
    assert_eq!(store2.get(CredentialKey::Session).as_deref(), Some("p=0"));
}

/// An injected store backs the session and a fresh session reloads it.
#[test]
fn test_injected_store_backs_session() {
    let (store, s) = session();
    s.save("sid=abc").unwrap();
    assert_eq!(
        store.get(CredentialKey::Session).as_deref(),
        Some("sid=abc")
    );
    assert!(s.state().is_signed_in);
    let reloaded = CookieAuthSession::new(config(&[], &[], None), store, clock());
    assert_eq!(reloaded.load_cookie_header().as_deref(), Some("sid=abc"));
    assert!(reloaded.state().is_signed_in);
}

/// Sign-out removes the credentials through the store but keeps the identity.
#[test]
fn test_sign_out_removes_credential_through_store() {
    let (store, s) = session();
    s.save("sid=abc").unwrap();
    s.save_account_email("user@example.com");
    s.sign_out();
    assert_eq!(store.get(CredentialKey::Session), None);
    assert_eq!(store.get(CredentialKey::Email), None);
    assert_eq!(
        store.get(CredentialKey::Account).as_deref(),
        Some("user@example.com")
    );
    assert!(s.state().needs_sign_in);
}

/// A refused write leaves the session signed out and reports the error.
#[test]
fn test_save_write_failure_is_reported() {
    let (store, s) = session();
    store.set_fail_writes(true);
    assert!(s.save("sid=abc").is_err());
    assert!(!s.state().is_signed_in);
    assert!(s.state().last_auth_error.is_some());
}

struct Cleaner(Mutex<u32>);
impl BrowserStateCleaner for Cleaner {
    fn clear_browser_state(&self, _: &AuthConfig) {
        *self.0.lock().unwrap() += 1;
    }
}

/// Sign-out and invalidation both clear the browser state.
#[test]
fn test_browser_state_cleared_on_sign_out_and_invalidation() {
    let cleaner = Arc::new(Cleaner(Mutex::new(0)));
    let s = CookieAuthSession::new(
        config(&[], &[], None),
        Arc::new(InMemoryCredentialStore::default()),
        clock(),
    )
    .with_browser_cleaner(cleaner.clone());
    s.save("sid=1").unwrap();
    s.sign_out();
    s.mark_session_invalid(None);
    assert_eq!(*cleaner.0.lock().unwrap(), 2);
}
