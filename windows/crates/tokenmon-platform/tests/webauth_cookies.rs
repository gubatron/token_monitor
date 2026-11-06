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
