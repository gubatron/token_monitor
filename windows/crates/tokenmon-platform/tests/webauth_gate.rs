//! Return gate, Back policy, navigation policy, profile folders
//! (spec 04 4.3-4.4, SignInBrowserNavigationTests).

#![allow(
    clippy::unwrap_used,
    clippy::unchecked_time_subtraction,
    clippy::cloned_ref_to_slice_refs,
    clippy::const_is_empty
)]

mod webauth_common;

use std::sync::Arc;

use tokenmon_platform::webauth::gate::{GateEvent, ReturnGate, is_navigation_allowed};
use tokenmon_platform::webauth::nav::{BackAction, NavSnapshot, NavState};
use tokenmon_platform::webauth::profile::ProfileStore;
use url::Url;
use webauth_common::claude_config;

fn u(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn gate(host_suffix: &'static str, return_host: &'static str) -> ReturnGate {
    ReturnGate::new(
        Arc::new(move |h, _| h.ends_with(host_suffix)),
        Arc::new(move |url| url.host_str() == Some(return_host)),
    )
}

// testReturnGateIgnoresStartURLUntilAuthHost
#[test]
fn start_url_is_ignored_until_an_auth_host_was_seen() {
    let mut g = gate("google.com", "cursor.com");
    assert_eq!(
        g.note(&u("https://cursor.com/dashboard/usage")),
        GateEvent::None
    );
    assert!(!g.did_see_auth());
    assert_eq!(
        g.note(&u("https://accounts.google.com/o/oauth")),
        GateEvent::AuthHost
    );
    assert!(g.did_see_auth());
    assert_eq!(
        g.note(&u("https://cursor.com/dashboard/usage")),
        GateEvent::ReturnPage
    );
}

// testReturnGateDoesNotCaptureBeforeAuth
#[test]
fn claude_new_before_login_is_not_a_return() {
    let c = claude_config();
    let mut g = ReturnGate::new(c.is_auth_host.clone(), c.is_return_page.clone());
    assert_eq!(g.note(&u("https://claude.ai/new")), GateEvent::None);
    assert_eq!(g.note(&u("https://claude.ai/login")), GateEvent::AuthHost);
    assert_eq!(g.note(&u("https://claude.ai/new")), GateEvent::ReturnPage);
    assert!(g.matches_return_page(&u("https://www.claude.ai/chat/abc")));
    assert!(!g.matches_return_page(&u("https://claude.ai/login")));
}

#[test]
fn a_url_that_is_both_auth_host_and_return_page_reports_return() {
    let mut g = ReturnGate::new(Arc::new(|_, _| true), Arc::new(|_| true));
    assert_eq!(g.note(&u("https://a.example/")), GateEvent::ReturnPage);
}

#[test]
fn gate_event_display_names() {
    assert_eq!(GateEvent::None.display_name(), "None");
    assert_eq!(GateEvent::AuthHost.display_name(), "Auth host");
    assert_eq!(GateEvent::ReturnPage.display_name(), "Return page");
}

#[test]
fn host_is_lowercased_before_the_predicate() {
    let mut g = ReturnGate::new(Arc::new(|h, _| h == "accounts.x.ai"), Arc::new(|_| false));
    assert_eq!(
        g.note(&u("https://ACCOUNTS.X.AI/sign-in")),
        GateEvent::AuthHost
    );
}

// testBackGoesBackWhenHistoryExists / ClosesPopup / DoesNothing
#[test]
fn back_policy() {
    assert_eq!(BackAction::decide(true, 0), BackAction::GoBack);
    assert_eq!(BackAction::decide(true, 1), BackAction::GoBack);
    assert_eq!(BackAction::decide(false, 1), BackAction::ClosePopup);
    assert_eq!(BackAction::decide(false, 2), BackAction::ClosePopup);
    assert_eq!(BackAction::decide(false, 0), BackAction::None);
}

// testBackActionRawValuesRoundTrip
#[test]
fn back_action_raw_values_round_trip() {
    for a in BackAction::ALL {
        assert_eq!(BackAction::from_raw(a.raw()), Some(a));
        assert_ne!(a.display_name(), "");
    }
    assert_eq!(BackAction::from_raw("sideways"), None);
    assert_eq!(BackAction::GoBack.display_name(), "Back");
    assert_eq!(BackAction::ClosePopup.display_name(), "Close popup");
    assert_eq!(BackAction::None.display_name(), "None");
}

#[test]
fn nav_state_derivations() {
    let mut n = NavState::default();
    assert!(!n.back_is_enabled());
    n.popup_depth = 1;
    assert!(n.has_popup() && n.back_is_enabled());
    assert_eq!(n.back_tooltip(), "Back to the sign-in page");
    assert_eq!(n.address_label(), "Sign-in popup");
    n.snapshot = NavSnapshot {
        can_go_back: true,
        current_url: Some("https://claude.ai/login?x=1".into()),
        ..NavSnapshot::default()
    };
    assert_eq!(n.back_tooltip(), "Back");
    n.popup_depth = 0;
    assert_eq!(n.address_label(), "claude.ai/login");
    n.snapshot.current_url = Some("about:blank".into());
    assert_eq!(n.address_label(), "about:blank");
    n.snapshot.current_url = None;
    assert_eq!(n.address_label(), "");
}

#[test]
fn only_https_navigation_is_allowed() {
    for ok in ["https://claude.ai/new", "about:blank"] {
        assert!(is_navigation_allowed(ok), "{ok}");
    }
    for bad in [
        "http://claude.ai/new",
        "http://localhost:8080/x",
        "file:///C:/Windows/win.ini",
        "javascript:alert(1)",
        "data:text/html,<script>1</script>",
        "blob:https://a.example/uuid",
        "ms-settings:privacy",
        "ftp://a.example/",
        "about:srcdoc",
        "https://",
        "",
        "not a url",
        "tokenmon://open",
        "view-source:https://a.example",
    ] {
        assert!(!is_navigation_allowed(bad), "{bad}");
    }
}

#[test]
fn profile_dirs_are_per_provider_and_per_session() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ProfileStore::new(tmp.path());
    let a = s.session_dir("claude", "s1").unwrap();
    let b = s.session_dir("chatgpt", "s1").unwrap();
    let c = s.session_dir("claude", "s2").unwrap();
    assert_ne!(a, b);
    assert_ne!(a, c);
    assert!(a.starts_with(s.provider_dir("claude").unwrap()));
    assert!(!b.starts_with(s.provider_dir("claude").unwrap()));
}

#[test]
fn profile_names_cannot_escape_the_root() {
    let s = ProfileStore::new("/data");
    for bad in [
        "",
        "..",
        "a/b",
        "a\\b",
        "A",
        "a.b",
        "c:",
        "../x",
        &"a".repeat(65),
        "a b",
        "\u{e9}",
    ] {
        assert!(s.provider_dir(bad).is_err(), "{bad:?}");
        assert!(s.session_dir("claude", bad).is_err(), "{bad:?}");
    }
    assert!(s.purge_provider("..").is_err());
}

#[test]
fn purge_removes_only_the_named_provider_and_tolerates_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ProfileStore::new(tmp.path().join("signin"));
    s.purge_provider("claude").unwrap();
    s.purge_all().unwrap();
    let a = s.session_dir("claude", "s1").unwrap();
    let b = s.session_dir("grok", "s1").unwrap();
    std::fs::create_dir_all(a.join("EBWebView/Default")).unwrap();
    std::fs::write(a.join("EBWebView/Default/Cookies"), b"x").unwrap();
    std::fs::create_dir_all(&b).unwrap();
    s.purge_provider("claude").unwrap();
    assert!(!a.exists());
    assert!(b.exists());
    s.purge_all().unwrap();
    assert!(!s.root().exists());
}

#[cfg(unix)]
#[test]
fn purge_does_not_follow_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), b"x").unwrap();
    let s = ProfileStore::new(tmp.path().join("signin"));
    std::fs::create_dir_all(s.root()).unwrap();
    std::os::unix::fs::symlink(&outside, s.root().join("claude")).unwrap();
    s.purge_provider("claude").unwrap();
    assert!(outside.join("keep.txt").exists());
    assert!(!s.root().join("claude").exists());
}

// testSignInBrowserDoesNotOverrideTheUserAgent (Windows: the adapter never
// sets a user agent, so WebView2's default Edge UA is used).
#[test]
fn adapter_never_overrides_the_user_agent_or_exposes_host_objects() {
    let src = include_str!("../src/webauth/webview2.rs");
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!code.contains("UserAgent"), "must not set a user agent");
    assert!(!code.contains("AddHostObjectToScript"), "no host objects");
    assert!(code.contains("SetAreHostObjectsAllowed(false)"));
    assert!(code.contains("SetIsWebMessageEnabled(false)"));
}

#[test]
fn navigation_policy_allows_only_provider_and_identity_hosts_over_https() {
    use tokenmon_platform::webauth::gate::NavigationPolicy;
    let p = NavigationPolicy::from_config(&claude_config());
    for ok in [
        "https://claude.ai/new",
        "https://www.claude.ai/chat/1",
        "https://clerk.claude.ai/sign-in",
        "https://CLAUDE.ai/new",
        "about:blank",
    ] {
        assert!(p.allows(ok), "{ok}");
    }
    for bad in [
        "http://claude.ai/new",
        "https://evil.example/",
        "https://evil.example/login",
        "https://claude.ai.evil.example/",
        "https://notclaude.ai/",
        "https://claude.ai@evil.example/",
        "https://evil.example/?u=https://claude.ai/",
        "https://evil.example/#claude.ai",
        "file:///C:/x",
        "",
    ] {
        assert!(!p.allows(bad), "{bad}");
    }
}

#[test]
fn grok_navigation_enforces_auth_hosts() {
    use tokenmon_core::net::Domain;
    use tokenmon_platform::webauth::gate::NavigationPolicy;
    const AUTH_HOSTS: [&str; 5] = [
        "accounts.x.ai",
        "auth.x.ai",
        "api.x.com",
        "twitter.com",
        "x.com",
    ];
    let mut cfg = webauth_common::grok_config();
    cfg.is_auth_host = Arc::new(|host, _| Domain::matches(host, &AUTH_HOSTS));
    cfg.policy.is_domain = Arc::new(|d| Domain::matches(d, &["grok.com", "x.ai"]));
    let p = NavigationPolicy::from_config(&cfg);
    for ok in [
        "https://accounts.x.ai/sign-in?redirect=https%3A%2F%2Fgrok.com%2F",
        "https://x.com/i/oauth2",
        "https://grok.com/",
    ] {
        assert!(p.allows(ok), "{ok}");
    }
    for bad in [
        "https://x.com.evil.example/",
        "https://evil.example/",
        "http://x.com/",
    ] {
        assert!(!p.allows(bad), "{bad}");
    }
}
