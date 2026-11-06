//! Credential sessions: one core [`CookieAuthSession`] per cookie provider,
//! all backed by the platform secret vault (spec 01 section 6.1 step 4).
//!
//! Cursor's session is shared with Grokbot, so Grokbot has no session of its
//! own. OpenRouter keeps its thin API-key session.

use std::path::Path;
use std::sync::Arc;

use tokenmon_core::{
    AuthConfig, BrowserStateCleaner, Clock, CookieAuthSession, CredentialStore,
    ProviderAuthSession, ProviderId,
};
use tokenmon_platform::secrets::{FallbackPolicy, SecretRoutingCredentialStore, Vault};
use tokenmon_platform::webauth::WebAuthHost;
use tokenmon_provider_openrouter::auth::OpenRouterAuthSession;

use crate::registry::SessionKey;

/// Store filename prefix of the OpenRouter key.
pub const OPENROUTER_STORE_PREFIX: &str = "openrouter_auth_";

/// The sign-in window slug of a provider: the key `WebAuthHost::sign_out` and
/// the window registry use. Grokbot signs in through Cursor.
pub fn slug(provider: ProviderId) -> &'static str {
    provider.sign_in_provider().as_str()
}

/// Clears the embedded browser's profile for the provider whose config it is
/// given (the platform host keys profiles by slug).
pub struct HostBrowserCleaner {
    host: Arc<dyn WebAuthHost>,
}

impl HostBrowserCleaner {
    /// A cleaner over `host`.
    pub fn new(host: Arc<dyn WebAuthHost>) -> Self {
        Self { host }
    }
}

impl BrowserStateCleaner for HostBrowserCleaner {
    fn clear_browser_state(&self, config: &AuthConfig) {
        let slug = ProviderId::USAGE_PROVIDERS
            .iter()
            .find(|p| auth_config_of(**p).is_some_and(|c| c.hosts == config.hosts))
            .map(|p| slug(*p));
        if let Some(slug) = slug {
            self.host.sign_out(slug);
        }
    }
}

/// The cookie-session config of a provider (`None` for key-based OpenRouter and
/// for Grokbot, which uses Cursor's).
pub fn auth_config_of(provider: ProviderId) -> Option<AuthConfig> {
    match provider {
        ProviderId::Grok => Some(tokenmon_provider_grok::auth::auth_config()),
        ProviderId::Cursor => Some(tokenmon_provider_cursor::auth::cursor_auth_config()),
        ProviderId::Claude => Some(tokenmon_provider_claude::auth::claude_auth_config()),
        ProviderId::ChatGpt => Some(tokenmon_provider_chatgpt::auth::chatgpt_auth_config()),
        ProviderId::OpenCode => Some(tokenmon_provider_opencode::auth::auth_config()),
        ProviderId::Overview | ProviderId::OpenRouter | ProviderId::Grokbot => None,
    }
}

/// Every session the app owns.
pub struct Sessions {
    /// grok.com.
    pub grok: Arc<CookieAuthSession>,
    /// cursor.com (shared with Grokbot).
    pub cursor: Arc<CookieAuthSession>,
    /// claude.ai.
    pub claude: Arc<CookieAuthSession>,
    /// chatgpt.com.
    pub chatgpt: Arc<CookieAuthSession>,
    /// opencode.ai console.
    pub opencode: Arc<CookieAuthSession>,
    /// OpenRouter API key.
    pub openrouter: Arc<OpenRouterAuthSession>,
}

impl Sessions {
    /// Opens every session over one shared `vault`.
    ///
    /// `dir` is the app data directory: it holds the legacy plain-file secrets
    /// that [`SecretRoutingCredentialStore::new`] migrates into the vault.
    pub fn open(
        dir: &Path,
        vault: &Arc<Vault>,
        policy: FallbackPolicy,
        clock: &Arc<dyn Clock>,
        cleaner: &Arc<dyn BrowserStateCleaner>,
    ) -> Self {
        let store = |prefix: &str| -> Arc<dyn CredentialStore> {
            Arc::new(SecretRoutingCredentialStore::new(
                Arc::clone(vault),
                dir,
                prefix,
                policy,
            ))
        };
        let cookie = |provider: ProviderId| -> Arc<CookieAuthSession> {
            let config = auth_config_of(provider).expect("cookie provider has an auth config");
            let store = store(&config.store_filename_prefix);
            Arc::new(
                CookieAuthSession::new(config, store, Arc::clone(clock))
                    .with_browser_cleaner(Arc::clone(cleaner)),
            )
        };
        Self {
            grok: cookie(ProviderId::Grok),
            cursor: cookie(ProviderId::Cursor),
            claude: cookie(ProviderId::Claude),
            chatgpt: cookie(ProviderId::ChatGpt),
            opencode: cookie(ProviderId::OpenCode),
            openrouter: Arc::new(OpenRouterAuthSession::new(store(OPENROUTER_STORE_PREFIX))),
        }
    }

    /// The writable cookie session behind `provider`'s sign-in (Grokbot maps to
    /// Cursor's); `None` for OpenRouter.
    pub fn cookie_session(&self, provider: ProviderId) -> Option<Arc<dyn ProviderAuthSession>> {
        match provider.sign_in_provider() {
            ProviderId::Grok => Some(self.grok.clone()),
            ProviderId::Cursor => Some(self.cursor.clone()),
            ProviderId::Claude => Some(self.claude.clone()),
            ProviderId::ChatGpt => Some(self.chatgpt.clone()),
            ProviderId::OpenCode => Some(self.opencode.clone()),
            _ => None,
        }
    }

    /// The auth state of the session `provider` authenticates with (Grokbot
    /// reports Cursor's).
    pub fn auth_state(&self, provider: ProviderId) -> tokenmon_core::AuthSessionState {
        use tokenmon_core::AuthSession;
        match provider.sign_in_provider() {
            ProviderId::Grok => self.grok.state(),
            ProviderId::Cursor => self.cursor.state(),
            ProviderId::Claude => self.claude.state(),
            ProviderId::ChatGpt => self.chatgpt.state(),
            ProviderId::OpenCode => self.opencode.state(),
            _ => self.openrouter.state(),
        }
    }

    /// Whether `provider` has a usable session (signed in and not asked to
    /// sign in again).
    pub fn signed_in(&self, provider: ProviderId) -> bool {
        let state = self.auth_state(provider);
        state.is_signed_in && !state.needs_sign_in
    }

    /// Identity of the session `provider` authenticates with.
    pub fn key(&self, provider: ProviderId) -> SessionKey {
        match provider.sign_in_provider() {
            ProviderId::Grok => SessionKey::of(&self.grok),
            ProviderId::Cursor => SessionKey::of(&self.cursor),
            ProviderId::Claude => SessionKey::of(&self.claude),
            ProviderId::ChatGpt => SessionKey::of(&self.chatgpt),
            ProviderId::OpenCode => SessionKey::of(&self.opencode),
            _ => SessionKey::of(&self.openrouter),
        }
    }
}
