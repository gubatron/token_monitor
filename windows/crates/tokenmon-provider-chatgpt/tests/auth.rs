//! Ports of the ChatGPT cases of `ProviderCookieAllowlistTests.swift` and
//! `SignInBrowserNavigationTests.swift`.
//!
//! The cookie capture pipeline (`WebKitCookieCapture.select`, spec 01 1.6.5) is
//! owned by ws-webauth; `select` below is a faithful test-local model of it so
//! the real ChatGPT policy (hosts, allowlist, preferred cookie) is exercised.
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]

use tokenmon_core::net::Domain;
use tokenmon_provider_chatgpt::{
    CHATGPT_HOSTS, chatgpt_auth_config, is_auth_host, is_preferred_session_cookie, is_return_page,
};

type Jar<'a> = [(&'a str, &'a str, &'a str)];

/// Spec 01 1.6.5 `select` for an allowlisted policy.
fn select(jar: &Jar<'_>) -> Option<Vec<String>> {
    let cfg = chatgpt_auth_config();
    let relevant: Vec<_> = jar
        .iter()
        .filter(|(_, _, domain)| Domain::matches(domain, &CHATGPT_HOSTS))
        .collect();
    let essential: Vec<_> = relevant
        .iter()
        .filter(|(n, _, _)| cfg.is_essential(n))
        .collect();
    essential
        .iter()
        .any(|(n, _, _)| is_preferred_session_cookie(n))
        .then(|| essential.iter().map(|(n, _, _)| (*n).to_owned()).collect())
}

/// `testChatGPTPolicyStoresOnlyTheSessionCookie`.
#[test]
fn test_policy_stores_only_the_session_cookie() {
    let jar = [
        ("__Secure-next-auth.session-token", "sess", "chatgpt.com"),
        ("_ga", "tracker", "chatgpt.com"),
        ("oai-did", "device", "chatgpt.com"),
    ];
    assert_eq!(select(&jar).unwrap(), ["__Secure-next-auth.session-token"]);
}

/// `testChatGPTPolicyKeepsChunkedSessionAndDropsCSRF`.
#[test]
fn test_policy_keeps_chunked_session_and_drops_csrf() {
    let jar = [
        ("__Host-next-auth.csrf-token", "csrf", "chatgpt.com"),
        ("__Secure-next-auth.session-token.0", "a", "chatgpt.com"),
        ("__Secure-next-auth.session-token.1", "b", "chatgpt.com"),
        ("_ga", "tracker", "chatgpt.com"),
    ];
    assert_eq!(
        select(&jar).unwrap(),
        [
            "__Secure-next-auth.session-token.0",
            "__Secure-next-auth.session-token.1"
        ]
    );
}

/// `testChatGPTPolicyRejectsCSRFOnlyJar`.
#[test]
fn test_policy_rejects_csrf_only_jar() {
    let jar = [
        ("__Host-next-auth.csrf-token", "csrf", "chatgpt.com"),
        ("_ga", "tracker", "chatgpt.com"),
    ];
    assert!(select(&jar).is_none());
}

/// Cookies from foreign domains are ignored, and the config matches the spec table.
#[test]
fn test_config_matches_spec_and_ignores_foreign_domains() {
    let jar = [("__Secure-next-auth.session-token", "x", "evil-chatgpt.com")];
    assert!(select(&jar).is_none());
    let cfg = chatgpt_auth_config();
    assert_eq!(cfg.store_filename_prefix, "chatgpt_auth_");
    assert_eq!(cfg.max_attempts, 4);
    assert_eq!(cfg.sign_out_hosts[0], "chatgpt.com");
    assert!(
        cfg.failure_message
            .starts_with("No ChatGPT session cookie found.")
    );
    assert!(is_preferred_session_cookie(
        "__SECURE-next-auth.session-token.12"
    ));
    assert!(!is_preferred_session_cookie(
        "__Secure-next-auth.session-tokenX"
    ));
}

/// `testChatGPTReturnPageExcludesLoginAndAuth` (provider slice).
#[test]
fn test_chatgpt_return_page_excludes_login_and_auth() {
    assert!(is_auth_host("auth.openai.com", "/"));
    assert!(is_auth_host("chatgpt.com", "/auth/login"));
    assert!(is_return_page("chatgpt.com", "/"));
    assert!(is_return_page("chatgpt.com", "/c/abc"));
    assert!(is_return_page("www.CHATGPT.com", "/c/abc"));
    assert!(!is_return_page("chatgpt.com", "/auth/login"));
    assert!(!is_return_page("chatgpt.com", "/login"));
    assert!(!is_return_page("auth.openai.com", "/"));
    assert!(!is_return_page("chatgpt.com.evil.example", "/"));
}
