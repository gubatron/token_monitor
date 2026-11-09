//! Port of OpenRouterAuthSessionTests.swift plus the three-strike rule.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use tokenmon_core::{AuthSession, CredentialKey, CredentialStore, InMemoryCredentialStore};
use tokenmon_provider_openrouter::{OpenRouterAuthSession, SaveKeyError};

fn session() -> (Arc<InMemoryCredentialStore>, OpenRouterAuthSession) {
    let store = Arc::new(InMemoryCredentialStore::default());
    let s = OpenRouterAuthSession::new(store.clone());
    (store, s)
}

/// A fresh session is signed out and prompts for a key.
#[test]
fn test_starts_signed_out() {
    let (_, s) = session();
    let st = s.state();
    assert!(!st.is_signed_in && st.needs_sign_in);
    assert!(s.api_key().is_none());
}

/// Non-key text is rejected; a padded key is saved trimmed.
#[test]
fn test_save_rejects_non_openrouter_key() {
    let (store, s) = session();
    assert_eq!(s.save_api_key("not-a-key"), Err(SaveKeyError::NotKeyShaped));
    assert_eq!(
        s.state().last_auth_error.as_deref(),
        Some("That doesn't look like an OpenRouter key (expected sk-or-…).")
    );
    assert_eq!(s.save_api_key("  \n"), Err(SaveKeyError::Empty));
    assert_eq!(
        s.state().last_auth_error.as_deref(),
        Some("Enter an OpenRouter API key.")
    );
    assert!(s.save_api_key("  sk-or-v1-abc123  \n").is_ok());
    assert_eq!(
        store.get(CredentialKey::ApiKey).as_deref(),
        Some("sk-or-v1-abc123")
    );
    let st = s.state();
    assert!(st.is_signed_in && !st.needs_sign_in && st.last_auth_error.is_none());
}

/// The key is stored as a secret credential and survives a new session.
#[test]
fn test_save_persists_across_instances() {
    let (store, s) = session();
    s.save_api_key("sk-or-v1-abc").unwrap();
    assert_eq!(
        CredentialKey::ApiKey.kind(),
        tokenmon_core::CredentialKind::Secret
    );
    let again = OpenRouterAuthSession::new(store);
    assert_eq!(again.api_key().unwrap().expose(), "sk-or-v1-abc");
    assert!(again.state().is_signed_in);
}

/// Sign-out removes the key.
#[test]
fn test_sign_out_clears_key() {
    let (store, s) = session();
    s.save_api_key("sk-or-v1-abc").unwrap();
    s.sign_out();
    assert!(store.get(CredentialKey::ApiKey).is_none());
    assert!(s.state().needs_sign_in);
}

/// Invalidation keeps the stored key but demands re-auth.
#[test]
fn test_mark_session_invalid_keeps_key_but_prompts_reauth() {
    let (_, s) = session();
    s.save_api_key("sk-or-v1-abc").unwrap();
    for _ in 0..2 {
        assert!(!s.record_auth_failure("rejected"));
    }
    assert!(s.record_auth_failure("rejected"));
    assert!(s.api_key().is_some());
    let st = s.state();
    assert!(st.needs_sign_in && !st.is_signed_in);
    assert_eq!(st.last_auth_error.as_deref(), Some("rejected"));
}

/// A rejected key is not current until replaced.
#[test]
fn test_rejected_key_is_not_current_until_replaced() {
    let (_, s) = session();
    s.save_api_key("sk-or-v1-abc").unwrap();
    let g = s.generation();
    assert!(s.is_current(g));
    for _ in 0..3 {
        s.record_auth_failure("x");
    }
    assert!(!s.is_current(g));
    s.save_api_key("sk-or-v1-new").unwrap();
    assert!(!s.is_current(g));
    assert!(s.is_current(s.generation()));
}

/// A failing store reports a fixed message and keeps the user signed out.
#[test]
fn test_store_failure_is_reported_without_the_key() {
    let (store, s) = session();
    store.set_fail_writes(true);
    let err = s.save_api_key("sk-or-v1-topsecret").unwrap_err();
    assert_eq!(err, SaveKeyError::StoreFailed);
    assert!(!err.to_string().contains("topsecret"));
    assert_eq!(err.to_string(), tokenmon_core::VAULT_UNAVAILABLE_MESSAGE);
    assert!(!s.state().is_signed_in);
}
