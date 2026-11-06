//! Shared fixtures: provider-shaped configs built from the spec tables.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use tokenmon_core::AuthConfig;
use tokenmon_platform::webauth::SignInConfig;
use tokenmon_platform::webauth::config::{DEFAULT_RETURN_DELAY, GROK_RETURN_DELAY};
use tokenmon_platform::webauth::cookies::{CapturePolicy, WebCookie};

pub fn host_matches(domain: &str, base: &str) -> bool {
    let d = domain.trim_start_matches('.').to_lowercase();
    d == base || d.ends_with(&format!(".{base}"))
}

pub fn auth(names: &[&str], prefixes: &[&str], preferred: &str, attempts: u32) -> AuthConfig {
    AuthConfig {
        store_filename_prefix: "x_auth_".into(),
        log_category: "x".into(),
        hosts: vec![],
        sign_out_hosts: vec!["example.com".into()],
        extra_store_keys: vec![],
        preferred_session_cookies: vec![preferred.into()],
        essential_cookie_names: names.iter().map(|s| (*s).to_owned()).collect(),
        essential_cookie_prefixes: prefixes.iter().map(|s| (*s).to_owned()).collect(),
        account_identity_cookie: None,
        max_attempts: attempts,
        retry_delay: AuthConfig::DEFAULT_RETRY_DELAY,
        failure_message: "No session yet.".into(),
    }
}

pub fn claude_policy() -> CapturePolicy {
    CapturePolicy::new(
        auth(&["sessionkey", "lastactiveorg"], &[], "sessionkey", 4),
        Arc::new(|d| host_matches(d, "claude.ai")),
    )
}

pub fn chatgpt_policy() -> CapturePolicy {
    CapturePolicy::new(
        auth(
            &[],
            &["__secure-next-auth.session-token"],
            "__secure-next-auth.session-token",
            4,
        ),
        Arc::new(|d| host_matches(d, "chatgpt.com")),
    )
}

pub fn grok_policy() -> CapturePolicy {
    CapturePolicy::new(
        tokenmon_provider_grok::auth::auth_config(),
        Arc::new(|d| host_matches(d, "grok.com") || host_matches(d, "x.ai")),
    )
}

pub fn opencode_policy() -> CapturePolicy {
    CapturePolicy::new(
        tokenmon_provider_opencode::auth::auth_config(),
        Arc::new(|d| host_matches(d, "opencode.ai")),
    )
}

/// Claude-shaped config: `claude.ai/login` or `clerk.claude.ai` are auth
/// hosts; `claude.ai/new` and `/chat/..` are return pages.
pub fn claude_config() -> SignInConfig {
    SignInConfig {
        provider_slug: "claude".into(),
        title: "Sign in to Claude".into(),
        subtitle: "Sign in to your Claude account.".into(),
        start_url: "https://claude.ai/new".into(),
        return_delay: DEFAULT_RETURN_DELAY,
        is_auth_host: Arc::new(|host, path| {
            host == "clerk.claude.ai"
                || host == "auth.claude.ai"
                || (host.ends_with("claude.ai")
                    && (path.starts_with("/login") || path.starts_with("/signin")))
        }),
        is_return_page: Arc::new(|u| {
            let h = u.host_str().unwrap_or("");
            (h == "claude.ai" || h == "www.claude.ai")
                && (u.path().starts_with("/new") || u.path().starts_with("/chat"))
        }),
        policy: claude_policy(),
    }
}

pub fn grok_config() -> SignInConfig {
    let mut c = claude_config();
    c.provider_slug = "grok".into();
    c.return_delay = GROK_RETURN_DELAY;
    c.policy = grok_policy();
    c.is_auth_host =
        Arc::new(|host, _| host == "accounts.x.ai" || host == "x.com" || host.ends_with(".x.com"));
    c.is_return_page = Arc::new(|u| u.host_str() == Some("grok.com"));
    c
}

pub fn cookie(name: &str, value: &str, domain: &str) -> WebCookie {
    WebCookie::new(name, value, domain)
}

pub fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}
