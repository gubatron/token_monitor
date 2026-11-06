//! Stable-toolchain regressions for the fuzz targets' hostile inputs
//! (`platform_webauth_email`, `platform_webauth_cookies`,
//! `platform_webauth_navigation`).

#![allow(clippy::unwrap_used)]

mod webauth_common;

use tokenmon_platform::webauth::cookies::{
    capture, extract_email, is_plausible_email, percent_decode,
};
use tokenmon_platform::webauth::gate::is_navigation_allowed;
use webauth_common::*;

#[test]
fn crlf_in_a_cookie_never_reaches_the_header() {
    let jar = vec![
        cookie("sessionKey", "v\r\nX: 1", ".claude.ai"),
        cookie("lastActiveOrg", "o", ".claude.ai"),
    ];
    // The session cookie is unsafe, so there is no session at all.
    assert!(capture(&jar, &claude_policy()).is_none());
}

#[test]
fn hostile_names_and_domains_do_not_panic() {
    let weird = [
        "",
        "\0",
        "\u{202e}",
        "a\tb",
        "é",
        &"x".repeat(100_000),
        "%",
        "%%%",
        "\u{fffd}",
    ];
    let mut jar = Vec::new();
    for n in weird {
        for d in weird {
            jar.push(cookie(n, n, d));
        }
    }
    let _ = capture(&jar, &claude_policy());
    let _ = capture(&jar, &chatgpt_policy());
}

#[test]
fn percent_decode_and_email_survive_pathological_input() {
    for s in [
        "%",
        "%%",
        "%%%%%%",
        "%0",
        "%00@a.com",
        "%FF%FE",
        "@",
        "@@",
        "a@b.c@d",
        &"%40".repeat(10_000),
        &"a".repeat(1_000_000),
    ] {
        let d = percent_decode(s);
        let _ = is_plausible_email(&d);
        let _ = extract_email(&[cookie("email", s, "d")]);
    }
    assert!(!is_plausible_email("a\0@b.com"));
}

#[test]
fn navigation_policy_rejects_scheme_tricks() {
    for bad in [
        " https://a.example",
        "HTTPS\u{0}://a.example",
        "jav\tascript:alert(1)",
        "\u{feff}https://a.example",
        "https:\\\\a.example\\@file",
        "//a.example/",
        "https://",
    ] {
        // Anything allowed must still be genuine http(s) with a host.
        if is_navigation_allowed(bad) {
            let u = url::Url::parse(bad).unwrap();
            assert!(matches!(u.scheme(), "http" | "https"), "{bad}");
            assert!(u.host_str().is_some(), "{bad}");
        }
    }
    assert!(!is_navigation_allowed("//a.example/"));
    assert!(!is_navigation_allowed("https://"));
}
