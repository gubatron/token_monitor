#![no_main]
//! `merge_refreshed_cookies` never panics on arbitrary stored headers and
//! `Set-Cookie` lists. The first byte picks the policy (allowlist or none),
//! `0x1F` separates the stored header from the `Set-Cookie` values.
//!
//! Invariants: output pieces are `name=value`; header-safe input gives
//! header-safe output (no CR/LF/NUL is introduced); a cookie not already
//! stored is added only when it is essential and was in the input; merging
//! the same headers again changes nothing.

use chrono::{TimeZone, Utc};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::AuthConfig;
use tokenmon_core::auth_session::{merge_refreshed_cookies, parse_refreshed_cookies};

fn config(with_allowlist: bool) -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "t_".into(),
        log_category: "Fuzz".into(),
        hosts: vec![],
        sign_out_hosts: vec!["example.com".into()],
        extra_store_keys: vec![],
        preferred_session_cookies: vec!["sso".into()],
        essential_cookie_names: if with_allowlist { vec!["sso".into()] } else { vec![] },
        essential_cookie_prefixes: if with_allowlist {
            vec!["__secure-next-auth.session-token".into()]
        } else {
            vec![]
        },
        account_identity_cookie: None,
        max_attempts: 1,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: String::new(),
    }
}

fn unsafe_byte(c: char) -> bool {
    matches!(c, '\r' | '\n' | '\0')
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let cfg = config(mode & 1 == 1);
    let text = String::from_utf8_lossy(rest);
    let mut fields = text.split('\u{1f}');
    let stored = fields.next().unwrap_or("");
    let headers: Vec<String> = fields.map(str::to_owned).collect();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let out = merge_refreshed_cookies(&cfg, stored, &headers, now);

    let clean_in = !stored.chars().any(unsafe_byte) && !headers.iter().any(|h| h.chars().any(unsafe_byte));
    if clean_in {
        assert!(!out.chars().any(unsafe_byte));
    }
    for piece in out.split("; ").filter(|p| !p.is_empty()) {
        assert!(piece.contains('='), "piece without '=': {piece:?}");
    }
    let stored_names: Vec<String> = stored
        .split(';')
        .map(|p| p.trim().split('=').next().unwrap_or("").to_lowercase())
        .collect();
    let refreshed = parse_refreshed_cookies(&headers, now);
    for piece in out.split("; ").filter(|p| !p.is_empty()) {
        let name = piece.split('=').next().unwrap_or("").trim().to_lowercase();
        if !stored_names.contains(&name) {
            assert!(cfg.is_essential(&name));
            assert!(refreshed.iter().any(|c| c.live && c.name.to_lowercase() == name));
        }
    }
    assert_eq!(merge_refreshed_cookies(&cfg, &out, &headers, now), out);
});
