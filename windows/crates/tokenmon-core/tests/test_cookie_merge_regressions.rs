//! Regressions found by the `core_cookie_merge` fuzz target.
#![allow(clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use tokenmon_core::AuthConfig;
use tokenmon_core::auth_session::merge_refreshed_cookies;

fn config() -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "t_".into(),
        log_category: "Fuzz".into(),
        hosts: vec![],
        sign_out_hosts: vec!["example.com".into()],
        extra_store_keys: vec![],
        preferred_session_cookies: vec!["sso".into()],
        essential_cookie_names: vec!["sso".into()],
        essential_cookie_prefixes: vec![],
        account_identity_cookie: None,
        max_attempts: 1,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: String::new(),
    }
}

/// A stored cookie whose name has trailing whitespace before `=` is kept
/// verbatim (the harness used to treat the padded name as a new cookie), and
/// merging again changes nothing.
#[test]
fn test_fuzz_padded_stored_name_is_preserved_and_stable() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let headers = vec!["sso=new; Path=/".to_owned()];
    let out = merge_refreshed_cookies(&config(), "other\t =x; sso=old", &headers, now);
    assert_eq!(out, "other\t =x; sso=new");
    assert_eq!(merge_refreshed_cookies(&config(), &out, &headers, now), out);
}
