//! Cookie selection, chunked tokens, e-mail extraction (spec 01 1.6.5).

#![allow(
    clippy::unwrap_used,
    clippy::unchecked_time_subtraction,
    clippy::cloned_ref_to_slice_refs,
    clippy::const_is_empty
)]

mod webauth_common;

use tokenmon_platform::webauth::cookies::{
    WebCookie, capture, cookie_header, extract_email, is_plausible_email, percent_decode,
    scoped_deletions, select,
};
use webauth_common::*;

#[test]
fn claude_allowlist_keeps_only_essential_and_needs_session_key() {
    let p = claude_policy();
    let jar = vec![
        cookie("sessionKey", "sk-1", ".claude.ai"),
        cookie("lastActiveOrg", "org-9", "claude.ai"),
        cookie("_ga", "junk", ".claude.ai"),
        cookie("sessionKey", "other", ".evil.example"),
    ];
    let got = select(&jar, &p).unwrap();
    let names: Vec<_> = got.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["sessionKey", "lastActiveOrg"]);
    assert_eq!(cookie_header(&got), "sessionKey=sk-1; lastActiveOrg=org-9");
}

#[test]
fn allowlist_without_preferred_cookie_is_not_a_session() {
    let p = claude_policy();
    let jar = vec![
        cookie("lastActiveOrg", "org", ".claude.ai"),
        cookie("_ga", "x", ".claude.ai"),
    ];
    assert!(select(&jar, &p).is_none());
}

#[test]
fn blank_session_cookie_is_not_a_session() {
    assert!(select(&[cookie("sessionKey", "", ".claude.ai")], &claude_policy()).is_none());
}

#[test]
fn nothing_for_the_provider_domain_gives_none() {
    let jar = vec![
        cookie("sessionKey", "v", ".notclaude.ai"),
        cookie("sessionKey", "v", "claude.ai.evil.example"),
    ];
    assert!(select(&jar, &claude_policy()).is_none());
    assert!(select(&[], &claude_policy()).is_none());
}

#[test]
fn http_only_cookies_are_captured() {
    let mut c = cookie("sso", "tok", ".grok.com");
    c.http_only = true;
    let got = capture(&[c], &grok_policy()).unwrap();
    assert!(got.cookies[0].http_only);
    assert_eq!(got.cookie_header, "sso=tok");
}

#[test]
fn chunked_next_auth_token_is_kept_in_full_and_counts_as_session() {
    let p = chatgpt_policy();
    let jar = vec![
        cookie("__Secure-next-auth.session-token.0", "AAA", ".chatgpt.com"),
        cookie("__Secure-next-auth.session-token.1", "BBB", ".chatgpt.com"),
        cookie("__Secure-next-auth.callback-url", "x", ".chatgpt.com"),
        cookie("oai-did", "y", ".chatgpt.com"),
    ];
    let got = capture(&jar, &p).unwrap();
    assert_eq!(
        got.cookie_header,
        "__Secure-next-auth.session-token.0=AAA; __Secure-next-auth.session-token.1=BBB"
    );
}

#[test]
fn unchunked_next_auth_token_works_and_lookalike_prefix_does_not() {
    let p = chatgpt_policy();
    assert!(
        select(
            &[cookie(
                "__Secure-next-auth.session-token",
                "T",
                "chatgpt.com"
            )],
            &p
        )
        .is_some()
    );
    // `...session-tokenX` is not the family.
    assert!(
        select(
            &[cookie(
                "__Secure-next-auth.session-tokenX",
                "T",
                "chatgpt.com"
            )],
            &p
        )
        .is_none()
    );
    // `.abc` is not a chunk index.
    assert!(
        select(
            &[cookie(
                "__Secure-next-auth.session-token.abc",
                "T",
                "chatgpt.com"
            )],
            &p
        )
        .is_none()
    );
}

#[test]
fn grok_accepts_sso_and_sso_rw_from_both_hosts() {
    let jar = vec![
        cookie("sso", "a", ".grok.com"),
        cookie("sso-rw", "b", ".x.ai"),
        cookie("auth_token", "x", ".x.com"),
    ];
    assert_eq!(
        cookie_header(&select(&jar, &grok_policy()).unwrap()),
        "sso=a; sso-rw=b"
    );
}

#[test]
fn opencode_needs_auth_cookie() {
    let jar = vec![
        cookie("auth", "a", "opencode.ai"),
        cookie("provider", "p", "auth.opencode.ai"),
    ];
    assert_eq!(select(&jar, &opencode_policy()).unwrap().len(), 2);
    assert!(
        select(
            &[cookie("provider", "p", "opencode.ai")],
            &opencode_policy()
        )
        .is_none()
    );
}

#[test]
fn no_allowlist_uses_preferred_then_auth_looking_cookies() {
    let mut p = claude_policy();
    p.auth.essential_cookie_names.clear();
    let jar = vec![
        cookie("sessionKey", "s", ".claude.ai"),
        cookie("misc", "m", ".claude.ai"),
    ];
    assert_eq!(select(&jar, &p).unwrap().len(), 2, "include-all default");
    p.include_all_domain_cookies_when_session_found = false;
    assert_eq!(select(&jar, &p).unwrap().len(), 1);

    let mut q = claude_policy();
    q.auth.essential_cookie_names.clear();
    q.auth.preferred_session_cookie = "nope".into();
    let only_misc = vec![
        cookie("misc", "m", ".claude.ai"),
        cookie("tok_auth", "t", ".claude.ai"),
    ];
    assert!(select(&only_misc, &q).is_none());
    q.looks_like_auth_cookie = std::sync::Arc::new(|n| n.contains("auth"));
    assert_eq!(select(&only_misc, &q).unwrap().len(), 2);
    q.include_all_domain_cookies_when_session_found = false;
    assert_eq!(select(&only_misc, &q).unwrap().len(), 1);
}

#[test]
fn header_unsafe_cookies_are_dropped() {
    let p = claude_policy();
    let jar = vec![
        cookie("sessionKey", "ok", ".claude.ai"),
        cookie("lastActiveOrg", "a\r\nX-Injected: 1", ".claude.ai"),
    ];
    assert_eq!(cookie_header(&select(&jar, &p).unwrap()), "sessionKey=ok");
    assert!(!cookie("a b", "v", "d").is_header_safe());
    assert!(!cookie("a", "v;w", "d").is_header_safe());
    assert!(!cookie("", "v", "d").is_header_safe());
    assert!(!cookie("a=b", "v", "d").is_header_safe());
    assert!(cookie("a", "b=c==", "d").is_header_safe());
}

#[test]
fn debug_output_never_contains_values() {
    let c = cookie("sessionKey", "SECRETVALUE", ".claude.ai");
    let got = capture(&[c.clone()], &claude_policy()).unwrap();
    assert!(!format!("{c:?}").contains("SECRETVALUE"));
    assert!(!format!("{got:?}").contains("SECRETVALUE"));
}

#[test]
fn attempts_and_message_defaults() {
    let mut p = claude_policy();
    assert_eq!(p.max_attempts(), 4);
    p.auth.max_attempts = 0;
    assert_eq!(p.max_attempts(), 1);
    assert_eq!(p.failure_message(), "No session yet.");
    p.auth.failure_message.clear();
    assert!(
        p.failure_message()
            .starts_with("Couldn't find a signed-in session")
    );
}

#[test]
fn email_prefers_named_cookies_then_any_plausible_value() {
    let jar = vec![
        cookie("x", "a%40b.com", "d"),
        cookie("user_email", "named%40example.org", "d"),
    ];
    assert_eq!(extract_email(&jar).as_deref(), Some("named@example.org"));
    assert_eq!(extract_email(&jar[..1]).as_deref(), Some("a@b.com"));
    assert_eq!(extract_email(&[cookie("email", "not-an-email", "d")]), None);
    assert_eq!(extract_email(&[]), None);
    // Named cookie with junk falls through to another plausible value.
    let jar = vec![cookie("email", "junk", "d"), cookie("y", "z@q.io", "d")];
    assert_eq!(extract_email(&jar).as_deref(), Some("z@q.io"));
}

#[test]
fn plausible_email_rules() {
    for ok in [
        "a@b.co",
        "first.last+tag@sub.example.com",
        "x_y@a-b.io",
        "!#$%&'*+-/=?^_`{|}~@a.bc",
    ] {
        assert!(is_plausible_email(ok), "{ok}");
    }
    let long_local = format!("{}@a.com", "a".repeat(65));
    let long_all = format!("a@{}.com", "b".repeat(250));
    for bad in [
        "",
        "a",
        "a@",
        "@b.com",
        "a@b",
        "a@@b.com",
        "a@b@c.com",
        "a b@c.com",
        "a@b..com",
        "a@-b.com",
        "a@b-.com",
        "a@b.c",
        "a@b.c0m",
        "a@b_c.com",
        "a@.com",
        "a@b.com.",
        &long_local,
        &long_all,
    ] {
        assert!(!is_plausible_email(bad), "{bad}");
    }
    assert!(is_plausible_email(&format!("{}@a.com", "a".repeat(64))));
}

#[test]
fn percent_decode_handles_edges() {
    assert_eq!(percent_decode("a%40b"), "a@b");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%4"), "%4");
    assert_eq!(percent_decode("%zz"), "%zz");
    assert_eq!(percent_decode("%C3%A9"), "\u{e9}");
    assert_eq!(percent_decode("%FF"), "\u{fffd}");
    assert_eq!(percent_decode(""), "");
    assert_eq!(percent_decode("%41"), "A");
}

#[test]
fn sign_out_deletions_are_scoped_to_the_provider() {
    let jar: Vec<WebCookie> = vec![
        cookie("sessionKey", "v", ".claude.ai"),
        cookie("g", "v", ".google.com"),
        cookie("c", "v", "claude.ai.evil.example"),
    ];
    let del = scoped_deletions(&jar, &|d| host_matches(d, "claude.ai"));
    assert_eq!(del.len(), 1);
    assert_eq!(del[0].name, "sessionKey");
    assert_eq!(del[0].domain, ".claude.ai");
}

// ---- ports of WebKitCookieCaptureTests / ProviderCookieAllowlistTests ----

#[test]
fn foreign_domain_cookies_are_ignored_by_every_allowlist_policy() {
    let foreign = |n: &str| {
        vec![
            cookie(n, "v", ".evil.example"),
            cookie(n, "v", "google.com"),
        ]
    };
    for (policy, name) in [
        (claude_policy(), "sessionKey"),
        (chatgpt_policy(), "__Secure-next-auth.session-token"),
        (grok_policy(), "sso"),
        (opencode_policy(), "auth"),
    ] {
        assert!(select(&foreign(name), &policy).is_none(), "{name}");
    }
}

#[test]
fn essential_names_narrow_what_is_stored() {
    let jar = vec![
        cookie("sso", "1", ".grok.com"),
        cookie("sso-rw", "2", ".grok.com"),
        cookie("cf_clearance", "3", ".grok.com"),
        cookie("_ga", "4", ".grok.com"),
    ];
    let names: Vec<_> = select(&jar, &grok_policy())
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["sso", "sso-rw"]);
}

#[test]
fn empty_allowlist_keeps_broad_capture() {
    let mut p = grok_policy();
    p.auth.essential_cookie_names.clear();
    let jar = vec![
        cookie("sso", "1", ".grok.com"),
        cookie("_ga", "4", ".grok.com"),
    ];
    assert_eq!(select(&jar, &p).unwrap().len(), 2);
}

#[test]
fn other_domain_cookies_are_ignored_but_provider_ones_kept() {
    let jar = vec![
        cookie("sso", "1", ".grok.com"),
        cookie("sso", "x", ".other.com"),
    ];
    assert_eq!(select(&jar, &grok_policy()).unwrap().len(), 1);
}

#[test]
fn extracts_email_named_cookie_decoded() {
    let c = cookie("email", "dev%40example.com", "d");
    assert_eq!(extract_email(&[c]).as_deref(), Some("dev@example.com"));
}

#[test]
fn extracts_user_email_named_cookie_case_insensitively() {
    let c = cookie("User_Email", "dev%40example.com", "d");
    assert_eq!(extract_email(&[c]).as_deref(), Some("dev@example.com"));
}

#[test]
fn falls_back_to_short_value_containing_at() {
    let c = cookie("account_hint", "me@site.io", "d");
    assert_eq!(extract_email(&[c]).as_deref(), Some("me@site.io"));
}

#[test]
fn rejects_implausible_email_values() {
    for bad in [
        "a@b",
        "@example.com",
        "user@",
        "x@y@z.com",
        "token@abc.12",
        "user@example.com; path=/",
        "\"q\"@example.com",
        "https://u@example.com",
        "not-an-email@localhost",
    ] {
        assert!(!is_plausible_email(bad), "{bad}");
        assert_eq!(extract_email(&[cookie("email", bad, "d")]), None, "{bad}");
    }
}

#[test]
fn ignores_long_values_with_spaces() {
    let v = format!("{} and more text @ example.com", "word ".repeat(80));
    assert_eq!(extract_email(&[cookie("note", &v, "d")]), None);
}

#[test]
fn returns_nil_without_email_like_cookies() {
    let jar = vec![cookie("a", "1", "d"), cookie("b", "plain", "d")];
    assert_eq!(extract_email(&jar), None);
}

#[test]
fn prefers_explicit_email_cookie_over_fallback() {
    let jar = vec![
        cookie("first", "other@x.io", "d"),
        cookie("email", "mine@y.io", "d"),
    ];
    assert_eq!(extract_email(&jar).as_deref(), Some("mine@y.io"));
}

#[test]
fn non_secure_cookies_are_never_captured_by_default() {
    let mut insecure = cookie("sessionKey", "tok", ".claude.ai");
    insecure.secure = false;
    let policy = claude_policy();
    assert!(policy.require_secure);
    assert!(capture(&[insecure.clone()], &policy).is_none());
    // A Secure session cookie is kept, a non-Secure sibling is dropped.
    let mut other = cookie("lastActiveOrg", "org", ".claude.ai");
    other.secure = false;
    let got = capture(&[cookie("sessionKey", "tok", ".claude.ai"), other], &policy).unwrap();
    assert_eq!(got.cookie_header, "sessionKey=tok");
    // Opting out restores the old behavior.
    let mut lax = claude_policy();
    lax.require_secure = false;
    assert!(capture(&[insecure], &lax).is_some());
}
