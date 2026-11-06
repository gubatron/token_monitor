//! Invariants: cookie selection never panics on arbitrary names, values and
//! domains; a captured header never contains control characters, and every
//! selected cookie belongs to the provider domain and is header-safe.
#![no_main]

use std::sync::Arc;
use std::time::Duration;

use libfuzzer_sys::fuzz_target;
use tokenmon_core::AuthConfig;
use tokenmon_platform::webauth::cookies::{CapturePolicy, WebCookie, capture};

fn policy() -> CapturePolicy {
    let auth = AuthConfig {
        store_filename_prefix: "f_".into(),
        log_category: "f".into(),
        hosts: vec![],
        sign_out_hosts: vec!["claude.ai".into()],
        extra_store_keys: vec![],
        preferred_session_cookies: vec!["__secure-next-auth.session-token".into()],
        essential_cookie_names: vec!["sessionkey".into()],
        essential_cookie_prefixes: vec!["__secure-next-auth.session-token".into()],
        account_identity_cookie: None,
        max_attempts: 4,
        retry_delay: Duration::from_millis(400),
        failure_message: String::new(),
    };
    CapturePolicy::new(
        auth,
        Arc::new(|d| d.trim_start_matches('.').ends_with("claude.ai")),
    )
}

fuzz_target!(|data: &str| {
    // One cookie per line: name TAB value TAB domain.
    let cookies: Vec<WebCookie> = data
        .split('\n')
        .map(|l| {
            let mut p = l.splitn(3, '\t');
            WebCookie::new(
                p.next().unwrap_or(""),
                p.next().unwrap_or(""),
                p.next().unwrap_or(""),
            )
        })
        .collect();
    if let Some(c) = capture(&cookies, &policy()) {
        assert!(!c.cookie_header.bytes().any(|b| b < b' ' || b == 0x7f));
        for k in &c.cookies {
            assert!(k.domain.trim_start_matches('.').ends_with("claude.ai"));
            assert!(k.is_header_safe());
        }
    }
});
