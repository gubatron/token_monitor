//! OpenCode sign-in policy (spec 03 section 3.1): hosts, cookie rules and
//! return-page detection. Pure functions; the WebView2 flow lives elsewhere.

use std::time::Duration;

use tokenmon_core::{AuthConfig, CredentialKey};

use crate::console::workspace_id_from_path;

/// Start URL of the sign-in window.
pub const START_URL: &str = "https://opencode.ai/console/login";

/// Lowercased name of the console session cookie.
pub const CONSOLE_SESSION_COOKIE: &str = "__host-console_session";

/// The OpenCode [`AuthConfig`].
///
/// `AuthConfig::preferred_session_cookie` holds one name, but OpenCode
/// accepts two; use [`is_preferred_session_cookie`] for the full rule.
pub fn auth_config() -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "opencode_auth_".into(),
        log_category: "OpenCodeAuth".into(),
        hosts: vec!["opencode.ai".into(), "auth.opencode.ai".into()],
        sign_out_hosts: vec!["opencode.ai".into(), "auth.opencode.ai".into()],
        extra_store_keys: vec![CredentialKey::Workspace],
        preferred_session_cookie: "auth".into(),
        essential_cookie_names: vec!["auth".into(), "provider".into(), CONSOLE_SESSION_COOKIE.into()],
        essential_cookie_prefixes: Vec::new(),
        account_identity_cookie: None,
        max_attempts: 4,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "No OpenCode console session cookie found. Finish signing in until you see the console, then click Finish Sign-In.".into(),
    }
}

/// `auth` or the console session cookie, case-insensitive.
pub fn is_preferred_session_cookie(name: &str) -> bool {
    let n = name.to_lowercase();
    n == "auth" || n == CONSOLE_SESSION_COOKIE
}

/// Whether a cookie name plausibly carries authentication.
pub fn looks_like_auth_cookie(name: &str) -> bool {
    let n = name.to_lowercase();
    ["auth", "session", "sid", CONSOLE_SESSION_COOKIE].contains(&n.as_str())
        || [
            "auth", "session", "token", "jwt", "sid", "account", "openid", "oauth",
        ]
        .iter()
        .any(|k| n.contains(k))
}

/// Host and path belong to the identity provider's login flow.
pub fn is_auth_host(host: &str, path: &str) -> bool {
    host.contains("auth.opencode.ai")
        || path.contains("/auth/authorize")
        || path.contains("/console/login")
}

/// Whether navigation reached a signed-in console page (host lowercased,
/// path case-sensitive).
pub fn is_return_page(host: &str, path: &str) -> bool {
    let host = host.to_lowercase();
    let opencode = host == "opencode.ai" || host.ends_with(".opencode.ai");
    if !opencode || host.contains("auth.") {
        return false;
    }
    if path.starts_with("/console") {
        return !path.starts_with("/console/login");
    }
    path.contains("/workspace") || (path.starts_with("/auth") && !path.contains("authorize"))
}

/// Workspace id to remember when the browser lands on `path`.
pub fn workspace_id_on_return(path: &str) -> Option<String> {
    workspace_id_from_path(path)
}

/// Retry delay used between cookie capture attempts.
pub const CAPTURE_RETRY_DELAY: Duration = AuthConfig::DEFAULT_RETRY_DELAY;
