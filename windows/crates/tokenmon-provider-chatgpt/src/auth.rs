//! ChatGPT sign-in and cookie-session configuration (spec 03 section 2.1).

use tokenmon_core::AuthConfig;

/// Hosts whose cookies belong to the ChatGPT session. Also the sign-out hosts;
/// the first is the base host for parsing refreshed cookies.
pub const CHATGPT_HOSTS: [&str; 5] = [
    "chatgpt.com",
    "www.chatgpt.com",
    "auth.openai.com",
    "auth0.openai.com",
    "api.openai.com",
];

/// Lowercase NextAuth session cookie family (a large JWT is chunked into
/// `<prefix>.0`, `<prefix>.1`, ...).
pub const SESSION_TOKEN_PREFIX: &str = "__secure-next-auth.session-token";

/// The [`AuthConfig`] of the ChatGPT cookie session. Only the session family is
/// essential; the CSRF cookie (present before sign-in) is excluded.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::chatgpt_auth_config;
/// let cfg = chatgpt_auth_config();
/// assert!(cfg.is_essential("__Secure-next-auth.session-token.1"));
/// assert!(!cfg.is_essential("__Host-next-auth.csrf-token"));
/// ```
pub fn chatgpt_auth_config() -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "chatgpt_auth_".to_owned(),
        log_category: "ChatGPTAuth".to_owned(),
        hosts: CHATGPT_HOSTS.map(str::to_owned).to_vec(),
        sign_out_hosts: CHATGPT_HOSTS.map(str::to_owned).to_vec(),
        extra_store_keys: Vec::new(),
        preferred_session_cookie: SESSION_TOKEN_PREFIX.to_owned(),
        essential_cookie_names: Vec::new(),
        essential_cookie_prefixes: vec![SESSION_TOKEN_PREFIX.to_owned()],
        account_identity_cookie: None,
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "No ChatGPT session cookie found. Finish signing in to chatgpt.com, \
                          then click Capture Session."
            .to_owned(),
    }
}

/// `isPreferredSessionCookie` / `looksLikeAuthCookie` live here because
/// `AuthConfig::is_preferred_session_cookie` only compares whole names while
/// ChatGPT also accepts the `.N` chunks.
///
/// True for the session cookie or one of its chunks (case-insensitive).
pub fn is_preferred_session_cookie(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == SESSION_TOKEN_PREFIX
        || lower
            .strip_prefix(SESSION_TOKEN_PREFIX)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// `looksLikeAuthCookie`: CSRF never counts; otherwise a name containing
/// `session-token` or `session_token`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::looks_like_auth_cookie;
/// assert!(looks_like_auth_cookie("__Secure-next-auth.session-token"));
/// assert!(!looks_like_auth_cookie("__Host-next-auth.csrf-token"));
/// assert!(!looks_like_auth_cookie("_ga"));
/// ```
pub fn looks_like_auth_cookie(name: &str) -> bool {
    let lower = name.to_lowercase();
    !lower.contains("csrf") && (lower.contains("session-token") || lower.contains("session_token"))
}

/// `isAuthHost(host:path:)`: OpenAI/Google/Apple/Microsoft identity hosts
/// (substring match on the host exactly like Swift) or an in-page login path.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::is_auth_host;
/// assert!(is_auth_host("auth.openai.com", "/"));
/// assert!(is_auth_host("chatgpt.com", "/auth/login"));
/// assert!(!is_auth_host("chatgpt.com", "/c/abc"));
/// ```
pub fn is_auth_host(host: &str, path: &str) -> bool {
    [
        "auth.openai",
        "auth0.openai",
        "accounts.google",
        "appleid.apple",
        "login.microsoft",
    ]
    .iter()
    .any(|needle| host.contains(needle))
        || path.contains("/login")
        || path.contains("/signin")
}

/// `isReturnPage`: on chatgpt.com (or a subdomain) and the lowercased path has
/// none of `login`, `signin`, `/auth`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::is_return_page;
/// assert!(is_return_page("chatgpt.com", "/c/abc"));
/// assert!(!is_return_page("chatgpt.com", "/auth/login"));
/// assert!(!is_return_page("auth.openai.com", "/"));
/// ```
pub fn is_return_page(host: &str, path: &str) -> bool {
    let host = host.to_lowercase();
    if host != "chatgpt.com" && !host.ends_with(".chatgpt.com") {
        return false;
    }
    let path = path.to_lowercase();
    !(path.contains("login") || path.contains("signin") || path.contains("/auth"))
}

/// Sign-in window constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChatGptSignIn;

impl ChatGptSignIn {
    /// Window title.
    pub const TITLE: &'static str = "Sign in to ChatGPT";
    /// Window subtitle.
    pub const SUBTITLE: &'static str = "Sign in to your ChatGPT account. This window finishes on its own once you return to chatgpt.com.";
    /// First page loaded.
    pub const START_URL: &'static str = "https://chatgpt.com/";
}
