//! Cursor sign-in and cookie-session configuration (spec 02 section 3.1).
//!
//! [`cursor_auth_config`] is the [`AuthConfig`] used by the Cursor session and,
//! unchanged, by Grokbot (which shares Cursor's session and its three-strike
//! counter). [`CursorSignIn`] carries the sign-in window constants and the two
//! URL classifiers the web-auth platform layer needs to detect when the user
//! has finished signing in.

use std::time::Duration;

use tokenmon_core::AuthConfig;
use tokenmon_core::net::Domain;

/// Hosts whose cookies belong to the Cursor session. Also the sign-out hosts.
/// The first entry is the base host used to parse refreshed cookies.
pub const CURSOR_COOKIE_HOSTS: [&str; 4] = [
    "cursor.com",
    "cursor.sh",
    "authenticator.cursor.sh",
    "api2.cursor.sh",
];

/// Identity-provider hosts that count as "still on a sign-in page".
pub const CURSOR_AUTH_HOSTS: [&str; 4] = [
    "authenticator.cursor.sh",
    "authenticator.cursor.com",
    "accounts.google.com",
    "github.com",
];

/// Lowercase name of the one cookie that carries the Cursor session.
pub const SESSION_COOKIE_NAME_LOWER: &str = "workoscursorsessiontoken";

/// The [`AuthConfig`] of the Cursor cookie session (also used by Grokbot).
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::cursor_auth_config;
/// let cfg = cursor_auth_config();
/// assert_eq!(cfg.store_filename_prefix, "cursor_auth_");
/// assert!(cfg.is_essential("WorkosCursorSessionToken"));
/// assert!(!cfg.is_essential("_ga"));
/// assert!(cfg.is_preferred_session_cookie("WorkosCursorSessionToken"));
/// ```
pub fn cursor_auth_config() -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "cursor_auth_".to_owned(),
        log_category: "CursorAuth".to_owned(),
        hosts: CURSOR_COOKIE_HOSTS.map(str::to_owned).to_vec(),
        sign_out_hosts: CURSOR_COOKIE_HOSTS.map(str::to_owned).to_vec(),
        extra_store_keys: Vec::new(),
        preferred_session_cookie: SESSION_COOKIE_NAME_LOWER.to_owned(),
        essential_cookie_names: vec![SESSION_COOKIE_NAME_LOWER.to_owned()],
        essential_cookie_prefixes: Vec::new(),
        account_identity_cookie: None,
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "No Cursor session cookie found. Finish signing in until you see the \
                          usage dashboard, then click Capture Session."
            .to_owned(),
    }
}

/// Sign-in window constants and page classifiers for Cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorSignIn;

impl CursorSignIn {
    /// Window title.
    pub const TITLE: &'static str = "Sign in to Cursor";
    /// Window subtitle.
    pub const SUBTITLE: &'static str = "Sign in to your Cursor account. This window finishes on its own once you reach the dashboard.";
    /// First page loaded.
    pub const START_URL: &'static str = "https://cursor.com/dashboard/usage";
    /// Delay between reaching the return page and capturing cookies.
    pub const RETURN_DELAY: Duration = Duration::from_millis(800);

    /// Whether `host`/`path` is an identity-provider page (the user is still
    /// signing in): an auth host, or any path containing `/login` or `/signin`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_provider_cursor::CursorSignIn;
    /// assert!(CursorSignIn::is_auth_page("github.com", "/session"));
    /// assert!(CursorSignIn::is_auth_page("cursor.com", "/login"));
    /// assert!(!CursorSignIn::is_auth_page("github.com.evil.example", "/"));
    /// assert!(!CursorSignIn::is_auth_page("cursor.com", "/dashboard"));
    /// ```
    pub fn is_auth_page(host: &str, path: &str) -> bool {
        Domain::matches(host, &CURSOR_AUTH_HOSTS)
            || path.contains("/login")
            || path.contains("/signin")
    }

    /// Whether the page is the post-sign-in return page: a cursor.com host and
    /// a path containing `/dashboard` or `/settings`.
    pub fn is_return_page(host: &str, path: &str) -> bool {
        let host = host.to_ascii_lowercase();
        (host == "cursor.com" || host.ends_with(".cursor.com"))
            && (path.contains("/dashboard") || path.contains("/settings"))
    }

    /// `isCursorDomain(d)`: whether a cookie domain belongs to Cursor.
    pub fn is_cursor_domain(domain: &str) -> bool {
        Domain::matches(domain, &CURSOR_COOKIE_HOSTS)
    }

    /// `looksLikeAuthCookie` (moot while the allowlist is configured).
    pub fn looks_like_auth_cookie(name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        lower == SESSION_COOKIE_NAME_LOWER
            || ["session", "token", "auth", "workos", "cursor"]
                .iter()
                .any(|needle| lower.contains(needle))
    }
}
