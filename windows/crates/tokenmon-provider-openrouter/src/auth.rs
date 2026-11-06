//! API-key session (`OpenRouterAuthSession`, spec 03 section 4.1).
//!
//! The key is a secret: it is stored under [`CredentialKey::ApiKey`] (routed
//! to the vault by the store) and only ever handled as an [`ApiKey`], whose
//! `Debug` output is redacted and which has no `Display`.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use tokenmon_core::{
    AuthEvent, AuthEventListener, AuthSession, AuthSessionState, CredentialKey, CredentialStore,
    SessionGeneration,
};

/// Prefixes an OpenRouter key starts with.
const KEY_PREFIXES: [&str; 2] = ["sk-or-v1-", "sk-or-"];

/// An OpenRouter API key. Redacted in `Debug`; never `Display`ed.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key without validating its shape.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The raw secret, for building the `Authorization` header only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Replaces every occurrence of the key in `text` with `[redacted]`.
    pub fn redact(&self, text: &str) -> String {
        if self.0.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.0, "[redacted]")
        }
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// Whether `key` is shaped like an OpenRouter key (`sk-or-v1-` or `sk-or-`).
///
/// # Examples
///
/// ```
/// use tokenmon_provider_openrouter::looks_like_api_key;
/// assert!(looks_like_api_key("sk-or-v1-abc"));
/// assert!(!looks_like_api_key("not-a-key"));
/// ```
pub fn looks_like_api_key(key: &str) -> bool {
    KEY_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// Why [`OpenRouterAuthSession::save_api_key`] refused a key. The messages
/// are user-facing and never contain the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveKeyError {
    /// Blank after trimming.
    Empty,
    /// Not `sk-or-...`.
    NotKeyShaped,
    /// The credential store rejected the write.
    StoreFailed,
}

impl fmt::Display for SaveKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "Enter an OpenRouter API key.",
            Self::NotKeyShaped => "That doesn't look like an OpenRouter key (expected sk-or-…).",
            Self::StoreFailed => "Could not store the API key.",
        })
    }
}

impl std::error::Error for SaveKeyError {}

/// Observable key session over a [`CredentialStore`].
pub struct OpenRouterAuthSession {
    store: Arc<dyn CredentialStore>,
    state: Mutex<AuthSessionState>,
    listeners: Mutex<Vec<Arc<dyn AuthEventListener>>>,
}

impl fmt::Debug for OpenRouterAuthSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenRouterAuthSession")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl OpenRouterAuthSession {
    /// Opens the session, signed in when a key is already stored.
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        let session = Self {
            store,
            state: Mutex::new(AuthSessionState::default()),
            listeners: Mutex::new(Vec::new()),
        };
        session.refresh_from_disk();
        session
    }

    /// The stored key, `None` when missing or empty.
    pub fn api_key(&self) -> Option<ApiKey> {
        self.store
            .get(CredentialKey::ApiKey)
            .filter(|k| !k.is_empty())
            .map(ApiKey::new)
    }

    /// Re-reads the store: failures reset, generation bumped.
    pub fn refresh_from_disk(&self) {
        let signed_in = self.api_key().is_some();
        let mut s = lock(&self.state);
        s.is_signed_in = signed_in;
        s.needs_sign_in = !signed_in;
        s.consecutive_auth_failures = 0;
        s.session_generation = s.session_generation.next();
    }

    /// Trims, validates and stores a pasted key.
    ///
    /// # Errors
    /// [`SaveKeyError`]; the message is also published as `last_auth_error`.
    pub fn save_api_key(&self, raw: &str) -> Result<(), SaveKeyError> {
        let trimmed = raw.trim();
        let outcome = if trimmed.is_empty() {
            Err(SaveKeyError::Empty)
        } else if !looks_like_api_key(trimmed) {
            Err(SaveKeyError::NotKeyShaped)
        } else {
            self.store
                .set(CredentialKey::ApiKey, trimmed)
                .map_err(|_| SaveKeyError::StoreFailed)
        };
        let mut s = lock(&self.state);
        match outcome {
            Ok(()) => s.mark_signed_in(None),
            Err(e) => s.last_auth_error = Some(e.to_string()),
        }
        outcome
    }

    /// Removes the key and fires [`AuthEvent::SignedOut`].
    pub fn sign_out(&self) {
        self.store.remove(CredentialKey::ApiKey);
        {
            let mut s = lock(&self.state);
            s.is_signed_in = false;
            s.needs_sign_in = true;
            s.last_auth_error = None;
            s.consecutive_auth_failures = 0;
            s.session_generation = s.session_generation.next();
        }
        let listeners: Vec<_> = lock(&self.listeners).clone();
        for l in listeners {
            l.on_auth_event(AuthEvent::SignedOut);
        }
    }

    /// Marks the poller as needing a key (no stored key found); keeps errors.
    pub fn set_needs_sign_in(&self, needs: bool) {
        lock(&self.state).needs_sign_in = needs;
    }

    /// Clears the "needs sign in" flag after a successful fetch.
    pub fn mark_fetch_succeeded(&self) {
        lock(&self.state).needs_sign_in = false;
    }

    /// The current generation.
    pub fn generation(&self) -> SessionGeneration {
        lock(&self.state).session_generation
    }
}

impl AuthSession for OpenRouterAuthSession {
    /// Always `None`: this provider authenticates with a bearer key, not
    /// cookies.
    fn cookie_header(&self) -> Option<String> {
        None
    }

    fn state(&self) -> AuthSessionState {
        lock(&self.state).clone()
    }

    fn record_auth_failure(&self, reason: &str) -> bool {
        lock(&self.state).record_auth_failure(reason)
    }

    fn record_auth_success(&self) {
        lock(&self.state).record_auth_success();
    }

    fn is_current(&self, generation: SessionGeneration) -> bool {
        lock(&self.state).is_current(generation)
    }

    fn add_listener(&self, listener: Arc<dyn AuthEventListener>) {
        lock(&self.listeners).push(listener);
    }
}
