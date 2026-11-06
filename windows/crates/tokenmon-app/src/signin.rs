//! Sign-in: one [`SignInConfig`] per provider and the delegate that adopts a
//! captured session (spec 04 section 4, spec 01 section 6.3).

use std::sync::Arc;
use std::time::Duration;

use tokenmon_core::net::Domain;
use tokenmon_core::{AuthConfig, CapturedSession, ProviderAuthSession, ProviderId};
use tokenmon_platform::webauth::gate::{AuthHostFn, ReturnPageFn};
use tokenmon_platform::webauth::{
    CapturePolicy, CapturedCookies, SignInConfig, SignInDelegate, WebAuthError, WebAuthHost,
};
use url::Url;

use crate::sessions::{Sessions, auth_config_of, slug};

/// Where to get the WebView2 runtime the sign-in window needs (https only).
pub const WEBVIEW2_DOWNLOAD_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

/// What the user is told when the sign-in window could not open.
///
/// ```
/// use tokenmon_app::signin::{sign_in_failure_message, WEBVIEW2_DOWNLOAD_URL};
/// use tokenmon_platform::webauth::WebAuthError;
/// let m = sign_in_failure_message(&WebAuthError::BrowserUnavailable("x".into()));
/// assert!(m.contains(WEBVIEW2_DOWNLOAD_URL));
/// ```
pub fn sign_in_failure_message(error: &WebAuthError) -> String {
    match error {
        WebAuthError::BrowserUnavailable(_) => format!(
            "Sign-in could not open: the Microsoft Edge WebView2 Runtime is missing or failed \
             to start. Install it from {WEBVIEW2_DOWNLOAD_URL} and try again."
        ),
        WebAuthError::Unsupported => "Sign-in is only available on Windows.".to_owned(),
        WebAuthError::InvalidConfig(detail) => {
            format!("Sign-in could not open: invalid sign-in configuration ({detail}).")
        }
    }
}

/// Called on the UI side after a sign-in finished (refresh pollers, update
/// panels). Receives the provider whose window was opened.
pub type SignInCompleted = Arc<dyn Fn(ProviderId) + Send + Sync>;

fn host_and_path(url: &Url) -> (String, String) {
    (
        url.host_str().unwrap_or_default().to_lowercase(),
        url.path().to_owned(),
    )
}

fn domain_predicate(config: &AuthConfig) -> impl Fn(&str) -> bool + Send + Sync + use<> {
    let hosts = config.hosts.clone();
    move |domain| {
        let hosts: Vec<&str> = hosts.iter().map(String::as_str).collect();
        Domain::matches(domain, &hosts)
    }
}

/// The sign-in window configuration of `provider`, or `None` when it has no
/// window (OpenRouter takes an API key instead).
///
/// Grokbot opens Cursor's window. Grok accepts both `sso` and `sso-rw`
/// (through its [`AuthConfig`] allowlist).
pub fn sign_in_config(provider: ProviderId) -> Option<SignInConfig> {
    let target = provider.sign_in_provider();
    let auth = auth_config_of(target)?;
    let mut policy;
    let (title, subtitle, start_url, delay): (&str, &str, &str, Duration);
    let is_auth_host: AuthHostFn;
    let is_return: ReturnPageFn;
    match target {
        ProviderId::Grok => {
            use tokenmon_provider_grok::auth as g;
            policy = CapturePolicy::new(auth.clone(), Arc::new(g::is_grok_domain));
            (title, subtitle, start_url, delay) = (
                g::SIGN_IN_TITLE,
                g::SIGN_IN_SUBTITLE,
                g::SIGN_IN_START_URL,
                g::RETURN_DELAY,
            );
            is_auth_host = Arc::new(|host, _| g::is_auth_host(host));
            is_return = Arc::new(|url| g::is_return_page(url.as_str()));
        }
        ProviderId::Cursor => {
            use tokenmon_provider_cursor::auth::CursorSignIn as c;
            policy = CapturePolicy::new(auth.clone(), Arc::new(c::is_cursor_domain));
            policy.looks_like_auth_cookie = Arc::new(c::looks_like_auth_cookie);
            (title, subtitle, start_url, delay) =
                (c::TITLE, c::SUBTITLE, c::START_URL, c::RETURN_DELAY);
            is_auth_host = Arc::new(c::is_auth_page);
            is_return = Arc::new(|url| {
                let (h, p) = host_and_path(url);
                c::is_return_page(&h, &p)
            });
        }
        ProviderId::Claude => {
            use tokenmon_provider_claude::auth as a;
            policy = CapturePolicy::new(auth.clone(), Arc::new(domain_predicate(&auth)));
            (title, subtitle, start_url, delay) = (
                a::SIGN_IN_TITLE,
                a::SIGN_IN_SUBTITLE,
                a::SIGN_IN_START_URL,
                Duration::from_millis(800),
            );
            is_auth_host = Arc::new(a::is_auth_host);
            is_return = Arc::new(|url| {
                let (h, p) = host_and_path(url);
                a::is_return_page(&h, &p)
            });
        }
        ProviderId::ChatGpt => {
            use tokenmon_provider_chatgpt::auth::{self as a, ChatGptSignIn as c};
            policy = CapturePolicy::new(auth.clone(), Arc::new(domain_predicate(&auth)));
            policy.looks_like_auth_cookie = Arc::new(a::looks_like_auth_cookie);
            (title, subtitle, start_url, delay) = (
                c::TITLE,
                c::SUBTITLE,
                c::START_URL,
                Duration::from_millis(800),
            );
            is_auth_host = Arc::new(a::is_auth_host);
            is_return = Arc::new(|url| {
                let (h, p) = host_and_path(url);
                a::is_return_page(&h, &p)
            });
        }
        ProviderId::OpenCode => {
            use tokenmon_provider_opencode::auth as a;
            policy = CapturePolicy::new(auth.clone(), Arc::new(domain_predicate(&auth)));
            policy.looks_like_auth_cookie = Arc::new(a::looks_like_auth_cookie);
            (title, subtitle, start_url, delay) = (
                "Sign in to OpenCode",
                "Sign in to the OpenCode console. This window finishes on its own once you reach your workspace.",
                a::START_URL,
                a::CAPTURE_RETRY_DELAY,
            );
            is_auth_host = Arc::new(a::is_auth_host);
            is_return = Arc::new(|url| {
                let (h, p) = host_and_path(url);
                a::is_return_page(&h, &p)
            });
        }
        _ => return None,
    }
    Some(SignInConfig {
        provider_slug: slug(provider).to_owned(),
        title: title.to_owned(),
        subtitle: subtitle.to_owned(),
        start_url: start_url.to_owned(),
        return_delay: delay,
        is_auth_host,
        is_return_page: is_return,
        policy,
    })
}

/// Adopts what the sign-in window captured into the provider's session and
/// reports completion.
struct SessionDelegate {
    provider: ProviderId,
    session: Arc<dyn ProviderAuthSession>,
    on_completed: SignInCompleted,
}

impl SignInDelegate for SessionDelegate {
    fn on_returned(&self, url: &str) {
        // OpenCode remembers the workspace id of the page the user landed on.
        if self.provider == ProviderId::OpenCode
            && let Ok(parsed) = Url::parse(url)
            && let Some(id) =
                tokenmon_provider_opencode::auth::workspace_id_on_return(parsed.path())
        {
            self.session.set_workspace_id(&id);
        }
    }

    fn adopt(&self, captured: CapturedCookies) -> Result<Option<String>, String> {
        let email = captured.email.clone();
        self.session
            .adopt(&CapturedSession {
                cookie_header: captured.cookie_header,
                email: email.clone(),
            })
            .map_err(|e| e.to_string())?;
        Ok(email)
    }

    fn on_complete(&self) {
        (self.on_completed)(self.provider);
    }
}

/// Opens and closes sign-in for every provider.
pub struct SignInService {
    host: Arc<dyn WebAuthHost>,
    sessions: Arc<Sessions>,
    on_completed: SignInCompleted,
}

impl SignInService {
    /// A service over `host`; `on_completed` runs after each successful sign-in.
    pub fn new(
        host: Arc<dyn WebAuthHost>,
        sessions: Arc<Sessions>,
        on_completed: SignInCompleted,
    ) -> Self {
        Self {
            host,
            sessions,
            on_completed,
        }
    }

    /// Opens `provider`'s sign-in window (Grokbot opens Cursor's). The
    /// completion callback receives the provider whose window it was.
    ///
    /// # Errors
    /// `InvalidConfig` for a provider without a window; the host's error when
    /// the embedded browser is unavailable.
    pub fn start(&self, provider: ProviderId) -> Result<(), WebAuthError> {
        let target = provider.sign_in_provider();
        let (Some(config), Some(session)) =
            (sign_in_config(target), self.sessions.cookie_session(target))
        else {
            return Err(WebAuthError::InvalidConfig(format!(
                "{provider} has no sign-in window"
            )));
        };
        let delegate = Arc::new(SessionDelegate {
            provider: target,
            session,
            on_completed: Arc::clone(&self.on_completed),
        });
        match self.host.open(config, delegate) {
            Ok(()) => {
                self.sessions.set_sign_in_error(provider, None);
                Ok(())
            }
            Err(e) => {
                tracing::warn!(error = %e, %provider, "sign-in could not open");
                self.sessions
                    .set_sign_in_error(provider, Some(sign_in_failure_message(&e)));
                Err(e)
            }
        }
    }

    /// Signs `provider` out: the session forgets its credentials (firing the
    /// sign-out event) and the browser profile is cleared.
    pub fn sign_out(&self, provider: ProviderId) {
        self.sessions.set_sign_in_error(provider, None);
        if let Some(session) = self.sessions.cookie_session(provider) {
            session.sign_out();
        } else if provider == ProviderId::OpenRouter {
            self.sessions.openrouter.sign_out();
        }
        self.host.sign_out(slug(provider));
    }
}
