//! Grok sign-in configuration (spec 02 section 1.1).
//!
//! The sign-in window itself is `tokenmon-platform`'s; this module holds the
//! provider-specific constants, the host and cookie policy as pure functions,
//! and the [`AuthConfig`] the shared session machinery consumes.

use std::time::Duration;

use tokenmon_core::AuthConfig;
use url::Url;

/// Title of the sign-in window.
pub const SIGN_IN_TITLE: &str = "Sign in to Grok";

/// Subtitle of the sign-in window.
pub const SIGN_IN_SUBTITLE: &str = "Sign in with your Grok / xAI account. This window finishes on its own once you're back on grok.com.";

/// Sign-in start URL (exact, percent-encoded).
pub const SIGN_IN_START_URL: &str =
    "https://accounts.x.ai/sign-in?redirect=https%3A%2F%2Fgrok.com%2F%3F_s%3Dusage";

/// Identity hosts allowed to load during sign-in; their cookies are never
/// captured.
pub const AUTH_HOSTS: [&str; 5] = ["accounts.x.ai", "auth.x.ai", "api.x.com", "twitter.com", "x.com"];

/// Session/cookie hosts, also the sign-out hosts.
pub const GROK_HOSTS: [&str; 2] = ["grok.com", "x.ai"];

/// Delay between returning to grok.com and capturing cookies.
pub const RETURN_DELAY: Duration = Duration::from_millis(1500);

/// Essential cookie names (lowercase).
pub const ESSENTIAL_COOKIES: [&str; 2] = ["sso", "sso-rw"];

/// Message shown when no session cookie was found.
pub const CAPTURE_FAILURE_MESSAGE: &str =
    "No session cookies found. Finish signing in, then click Capture Session.";

/// `Domain.matches`: case-insensitive; one leading `.` is stripped; matches
/// when the host equals an entry or ends with `"." + entry`. A substring
/// never matches.
///
/// A private stand-in for the shared core-net helper.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::auth::domain_matches;
/// assert!(domain_matches(".grok.com", &["grok.com"]));
/// assert!(!domain_matches("evilgrok.com", &["grok.com"]));
/// ```
pub fn domain_matches(host: &str, entries: &[&str]) -> bool {
    let host = host.to_lowercase();
    let host = host.strip_prefix('.').unwrap_or(&host);
    entries.iter().any(|entry| {
        let entry = entry.to_lowercase();
        host == entry || host.ends_with(&format!(".{entry}"))
    })
}

/// Whether a cookie domain belongs to the Grok session.
pub fn is_grok_domain(domain: &str) -> bool {
    domain_matches(domain, &GROK_HOSTS)
}

/// Whether a host is an identity host allowed during sign-in.
pub fn is_auth_host(host: &str) -> bool {
    domain_matches(host, &AUTH_HOSTS)
}

/// Whether `url` is the post-sign-in return page: any URL whose host is
/// `grok.com` or a subdomain. `grok.com.evil.example` is rejected.
pub fn is_return_page(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| domain_matches(h, &["grok.com"])))
        .unwrap_or(false)
}

/// Whether a cookie name is one of the preferred session cookies
/// (`sso`, `sso-rw`), case-insensitively.
///
/// [`AuthConfig`] holds a single `preferred_session_cookie` (`sso`); the sign-in
/// window must use this function so a session with only `sso-rw` is accepted.
pub fn is_preferred_session_cookie(name: &str) -> bool {
    ESSENTIAL_COOKIES.contains(&name.to_lowercase().as_str())
}

/// The `AuthConfig` of the Grok session.
///
/// # Examples
///
/// ```
/// let cfg = tokenmon_provider_grok::auth::auth_config();
/// assert_eq!(cfg.store_filename_prefix, "auth_");
/// assert!(cfg.is_essential("SSO-RW"));
/// ```
pub fn auth_config() -> AuthConfig {
    let owned = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    AuthConfig {
        store_filename_prefix: "auth_".to_owned(),
        log_category: "Auth".to_owned(),
        hosts: owned(&GROK_HOSTS),
        sign_out_hosts: owned(&GROK_HOSTS),
        extra_store_keys: Vec::new(),
        preferred_session_cookie: "sso".to_owned(),
        essential_cookie_names: owned(&ESSENTIAL_COOKIES),
        essential_cookie_prefixes: Vec::new(),
        account_identity_cookie: None,
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: CAPTURE_FAILURE_MESSAGE.to_owned(),
    }
}

/// A cookie read from the sign-in browser's jar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieRecord {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Cookie domain (may start with `.`).
    pub domain: String,
}

/// Builds the stored `Cookie` header from a browser jar, or `None` when no
/// session cookie is present.
///
/// Keeps cookies of Grok domains, only the essential names, and succeeds only
/// when a preferred one (`sso` / `sso-rw`) is among them. X/Twitter session
/// cookies in the same jar are never persisted.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::auth::{select_session_cookies, CookieRecord};
/// let jar = [
///     CookieRecord { name: "sso".into(), value: "s".into(), domain: "grok.com".into() },
///     CookieRecord { name: "auth_token".into(), value: "x".into(), domain: "x.com".into() },
/// ];
/// assert_eq!(select_session_cookies(&jar).as_deref(), Some("sso=s"));
/// ```
pub fn select_session_cookies(jar: &[CookieRecord]) -> Option<String> {
    let essential: Vec<&CookieRecord> = jar
        .iter()
        .filter(|c| is_grok_domain(&c.domain))
        .filter(|c| ESSENTIAL_COOKIES.contains(&c.name.to_lowercase().as_str()))
        .collect();
    if !essential.iter().any(|c| is_preferred_session_cookie(&c.name)) {
        return None;
    }
    Some(
        essential
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; "),
    )
}
