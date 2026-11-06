//! Contract tests for core auth types.

use std::time::Duration;

use tokenmon_core::{
    AUTH_FAILURE_THRESHOLD, AuthConfig, AuthSessionState, CredentialKey, CredentialKind,
    CredentialStore, CredentialStoreError, InMemoryCredentialStore, SessionGeneration,
};

fn chatgpt_like() -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "chatgpt_auth_".into(),
        log_category: "chatgpt".into(),
        hosts: vec!["chatgpt.com".into()],
        sign_out_hosts: vec!["chatgpt.com".into()],
        extra_store_keys: vec![],
        preferred_session_cookies: vec!["__secure-next-auth.session-token".into()],
        essential_cookie_names: vec![],
        essential_cookie_prefixes: vec!["__secure-next-auth.session-token".into()],
        account_identity_cookie: None,
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "no session".into(),
    }
}

/// The threshold constant is the spec's 3.
#[test]
fn test_threshold_is_three() {
    assert_eq!(AUTH_FAILURE_THRESHOLD, 3);
    assert_eq!(AuthConfig::DEFAULT_RETRY_DELAY, Duration::from_millis(400));
}

/// Secret keys route to the vault, the rest to files.
#[test]
fn test_credential_key_routing() {
    assert_eq!(CredentialKey::Session.kind(), CredentialKind::Secret);
    assert_eq!(CredentialKey::ApiKey.kind(), CredentialKind::Secret);
    for k in [
        CredentialKey::Email,
        CredentialKey::Account,
        CredentialKey::Workspace,
    ] {
        assert_eq!(k.kind(), CredentialKind::Plain);
    }
    assert_eq!(CredentialKey::Session.as_str(), "session");
    assert_eq!(CredentialKey::ApiKey.as_str(), "key");
}

/// The in-memory store gets, sets, overwrites and removes; failures inject.
#[test]
fn test_in_memory_store_behaviour() {
    let s = InMemoryCredentialStore::default();
    assert_eq!(s.get(CredentialKey::Email), None);
    s.set(CredentialKey::Email, "a@b.co").unwrap();
    s.set(CredentialKey::Email, "c@d.co").unwrap();
    assert_eq!(s.get(CredentialKey::Email).as_deref(), Some("c@d.co"));
    s.remove(CredentialKey::Email);
    s.remove(CredentialKey::Email);
    assert_eq!(s.get(CredentialKey::Email), None);

    s.set_fail_writes(true);
    assert!(matches!(
        s.set(CredentialKey::Session, "x"),
        Err(CredentialStoreError::WriteFailed(_))
    ));
    assert_eq!(s.get(CredentialKey::Session), None);
}

/// Essential names/prefixes: prefix family includes `prefix` and `prefix.N`.
#[test]
fn test_essential_allowlist_helpers() {
    let c = chatgpt_like();
    assert!(c.has_essential_allowlist());
    assert!(c.is_essential("__Secure-next-auth.session-token"));
    assert!(c.is_essential("__secure-next-auth.session-token.0"));
    assert!(!c.is_essential("__secure-next-auth.session-tokenX"));
    assert!(!c.is_essential("theme"));
    assert_eq!(
        c.essential_family("__secure-next-auth.session-token.1"),
        Some("__secure-next-auth.session-token")
    );
    let none = AuthConfig {
        essential_cookie_prefixes: vec![],
        ..c
    };
    assert!(!none.has_essential_allowlist());
}

/// Third consecutive failure invalidates; success resets the streak.
#[test]
fn test_three_strikes_invalidate_session() {
    let mut s = AuthSessionState::default();
    assert!(s.needs_sign_in && !s.is_signed_in);
    s.mark_signed_in(Some("a@b.co".into()));
    assert!(!s.record_auth_failure("r"));
    assert!(!s.record_auth_failure("r"));
    s.record_auth_success();
    assert_eq!(s.consecutive_auth_failures, 0);
    assert!(!s.record_auth_failure("r"));
    assert!(!s.record_auth_failure("r"));
    assert!(s.record_auth_failure("session rejected"));
    assert!(s.needs_sign_in && !s.is_signed_in);
    assert_eq!(s.account_email, None);
    assert_eq!(s.last_auth_error.as_deref(), Some("session rejected"));
    assert_eq!(s.consecutive_auth_failures, 0);
}

/// The generation is monotonic and gates `is_current`.
#[test]
fn test_generation_monotonic_and_is_current() {
    let mut s = AuthSessionState::default();
    let g0 = s.session_generation;
    s.mark_signed_in(None);
    let g1 = s.session_generation;
    assert!(g1 > g0);
    assert!(s.is_current(g1));
    assert!(!s.is_current(g0));
    s.mark_session_invalid(None);
    assert!(s.session_generation > g1);
    assert!(!s.is_current(s.session_generation));
    assert_eq!(
        SessionGeneration(u64::MAX).next(),
        SessionGeneration(u64::MAX)
    );
}

/// Several preferred session cookie names are each accepted, case-insensitively.
#[test]
fn test_is_preferred_session_cookie_accepts_any_listed_name() {
    let mut cfg = chatgpt_like();
    cfg.preferred_session_cookies = vec!["sso".into(), "sso-rw".into()];
    assert!(cfg.is_preferred_session_cookie("SSO-RW"));
    assert!(cfg.is_preferred_session_cookie("sso"));
    assert!(!cfg.is_preferred_session_cookie("other"));
}
