//! Claude sign-in rules (`ClaudeAuthSession`, `ClaudeSignIn`).

use tokenmon_core::AuthConfig;
use tokenmon_core::net::Domain;

/// Where the sign-in window starts.
pub const SIGN_IN_START_URL: &str = "https://claude.ai/new";
/// Sign-in window title.
pub const SIGN_IN_TITLE: &str = "Sign in to Claude";
/// Sign-in window subtitle.
pub const SIGN_IN_SUBTITLE: &str =
    "Sign in to your Claude account. This window finishes on its own once you return to claude.ai.";
/// Hosts whose cookies belong to Claude.
pub const CLAUDE_HOSTS: [&str; 3] = ["claude.ai", "www.claude.ai", "api.claude.ai"];

/// The Claude [`AuthConfig`]: cookie allowlist `sessionKey` and
/// `lastActiveOrg` (case-insensitive), `lastActiveOrg` as the account
/// identity, four capture attempts.
pub fn claude_auth_config() -> AuthConfig {
    let hosts: Vec<String> = CLAUDE_HOSTS.iter().map(|h| (*h).to_owned()).collect();
    AuthConfig {
        store_filename_prefix: "claude_auth_".to_owned(),
        log_category: "ClaudeAuth".to_owned(),
        hosts: hosts.clone(),
        sign_out_hosts: hosts,
        extra_store_keys: Vec::new(),
        preferred_session_cookie: "sessionkey".to_owned(),
        essential_cookie_names: vec!["sessionkey".to_owned(), "lastactiveorg".to_owned()],
        essential_cookie_prefixes: Vec::new(),
        account_identity_cookie: Some("lastactiveorg".to_owned()),
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "No Claude session cookie found. Finish signing in to claude.ai, then click Capture Session.".to_owned(),
    }
}

/// Pages that must be allowed to load during sign-in but are never captured:
/// identity-provider hosts and any `/login` or `/signin` path.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::auth::is_auth_host;
/// assert!(is_auth_host("clerk.claude.ai", "/sign-in"));
/// assert!(is_auth_host("claude.ai", "/login"));
/// assert!(!is_auth_host("claude.ai", "/new"));
/// assert!(!is_auth_host("evil.example", "/login"));
/// assert!(!is_auth_host("github.com.evil.example", "/"));
/// ```
pub fn is_auth_host(host: &str, path: &str) -> bool {
    let p = path.to_lowercase();
    Domain::matches(host, &AUTH_IDP_HOSTS)
        || (Domain::matches(host, &["claude.ai"])
            && (p.contains("/login") || p.contains("/signin")))
}

/// Identity-provider hosts of the Claude sign-in, matched exactly or by
/// dot-suffix (never by substring: `github.com.evil.example` is not GitHub).
pub const AUTH_IDP_HOSTS: [&str; 5] = [
    "clerk.claude.ai",
    "accounts.google.com",
    "github.com",
    "appleid.apple.com",
    "idmsa.apple.com",
];

/// Whether landing on `host`/`path` means sign-in finished and the session
/// should be captured: exactly `claude.ai` or `www.claude.ai`, and a path
/// without `login`, `signin` or `/auth`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_claude::auth::is_return_page;
/// assert!(is_return_page("claude.ai", "/new"));
/// assert!(!is_return_page("claude.ai", "/auth/callback"));
/// assert!(!is_return_page("auth.claude.ai", "/new"));
/// ```
pub fn is_return_page(host: &str, path: &str) -> bool {
    let h = host.to_lowercase();
    let p = path.to_lowercase();
    (h == "claude.ai" || h == "www.claude.ai")
        && !(p.contains("login") || p.contains("signin") || p.contains("/auth"))
}
