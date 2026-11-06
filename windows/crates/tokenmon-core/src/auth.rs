//! Credential storage and cookie-session contracts (spec 01 sections 1.6.1 and
//! 1.6.4).
//!
//! Types and traits only, plus the small pure pieces every implementation
//! shares ([`AuthSessionState`], [`AuthConfig`] cookie helpers) and an
//! in-memory [`CredentialStore`] for tests. The DPAPI vault, secret routing
//! and WebView2 cookie capture are owned by ws-secrets and ws-webauth.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use thiserror::Error;

/// Consecutive auth rejections tolerated before the session is invalidated.
pub const AUTH_FAILURE_THRESHOLD: u32 = 3;

/// Whether a stored value is a secret (encrypted vault) or plain (file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialKind {
    /// Goes to the DPAPI vault.
    Secret,
    /// Goes to a plain owner-only file.
    Plain,
}

/// Typed key of a stored credential value (replaces the Swift string keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CredentialKey {
    /// The `Cookie` header value for the provider. Secret.
    Session,
    /// Signed-in account email. Plain.
    Email,
    /// Account identity used to detect an account switch. Plain.
    Account,
    /// OpenCode workspace id. Plain.
    Workspace,
    /// OpenRouter API key (store prefix `openrouter_auth_`). Secret.
    ApiKey,
}

impl CredentialKey {
    /// Storage name (file name suffix and vault entry suffix).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Email => "email",
            Self::Account => "account",
            Self::Workspace => "workspace",
            Self::ApiKey => "key",
        }
    }

    /// Secret keys are routed to the vault; the rest to plain files.
    pub const fn kind(self) -> CredentialKind {
        match self {
            Self::Session | Self::ApiKey => CredentialKind::Secret,
            Self::Email | Self::Account | Self::Workspace => CredentialKind::Plain,
        }
    }
}

/// A credential could not be written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CredentialStoreError {
    /// The backing store refused or failed the write.
    #[error("credential store write failed: {0}")]
    WriteFailed(String),
}

/// Key/value storage for credentials of one provider (`CredentialStore`).
///
/// Reads are infallible by design: an unreadable value is "not stored".
pub trait CredentialStore: Send + Sync {
    /// The stored value, if any.
    fn get(&self, key: CredentialKey) -> Option<String>;

    /// Stores `value`, replacing any previous one.
    fn set(&self, key: CredentialKey, value: &str) -> Result<(), CredentialStoreError>;

    /// Removes the value; removing a missing key is not an error.
    fn remove(&self, key: CredentialKey);
}

/// In-memory [`CredentialStore`] for tests (`InMemoryCredentialStore`).
///
/// # Examples
///
/// ```
/// use tokenmon_core::{CredentialKey, CredentialStore, InMemoryCredentialStore};
///
/// let store = InMemoryCredentialStore::default();
/// store.set(CredentialKey::Session, "a=b").unwrap();
/// assert_eq!(store.get(CredentialKey::Session).as_deref(), Some("a=b"));
/// store.remove(CredentialKey::Session);
/// assert_eq!(store.get(CredentialKey::Session), None);
/// ```
#[derive(Default)]
pub struct InMemoryCredentialStore {
    values: Mutex<HashMap<CredentialKey, String>>,
    fail_writes: AtomicBool,
}

impl std::fmt::Debug for InMemoryCredentialStore {
    /// Stored values are credentials: never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryCredentialStore")
            .field("values", &"<redacted>")
            .field("fail_writes", &self.fail_writes)
            .finish()
    }
}

impl InMemoryCredentialStore {
    /// Makes every subsequent [`CredentialStore::set`] fail (or succeed again),
    /// to test write-failure paths.
    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn get(&self, key: CredentialKey) -> Option<String> {
        self.values.lock().expect("store lock").get(&key).cloned()
    }

    fn set(&self, key: CredentialKey, value: &str) -> Result<(), CredentialStoreError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(CredentialStoreError::WriteFailed("injected failure".into()));
        }
        self.values
            .lock()
            .expect("store lock")
            .insert(key, value.to_owned());
        Ok(())
    }

    fn remove(&self, key: CredentialKey) {
        self.values.lock().expect("store lock").remove(&key);
    }
}

/// Per-provider auth configuration (`ProviderAuthConfig` + cookie policy).
///
/// Names are stored lowercase; use the matching helpers rather than comparing
/// by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    /// Prefix of this provider's store entries (`grok_auth_`, ...).
    pub store_filename_prefix: String,
    /// Log category for tracing.
    pub log_category: String,
    /// Hosts whose cookies belong to this provider (matched with
    /// `Domain::matches`, owned by ws-core-net).
    pub hosts: Vec<String>,
    /// Hosts to clear on sign-out; the first is the base host for parsing
    /// refreshed cookies.
    pub sign_out_hosts: Vec<String>,
    /// Extra keys cleared with the session (for example `Workspace`).
    pub extra_store_keys: Vec<CredentialKey>,
    /// Lowercase name of the cookie that proves a session (for example
    /// `sessionkey`).
    pub preferred_session_cookie: String,
    /// Lowercase essential cookie names; empty with no prefixes means "no
    /// allowlist".
    pub essential_cookie_names: Vec<String>,
    /// Lowercase essential prefixes (`prefix` itself and `prefix.N` chunks).
    pub essential_cookie_prefixes: Vec<String>,
    /// Cookie whose value identifies the account (Claude: `lastactiveorg`);
    /// `None` means the email identifies it.
    pub account_identity_cookie: Option<String>,
    /// Capture attempts (providers use 4).
    pub max_attempts: u32,
    /// Delay between capture attempts (default 400 ms).
    pub retry_delay: Duration,
    /// Shown when capture finds no session.
    pub failure_message: String,
}

impl AuthConfig {
    /// Default capture retry delay.
    pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(400);

    /// Whether an essential allowlist is configured.
    pub fn has_essential_allowlist(&self) -> bool {
        !self.essential_cookie_names.is_empty() || !self.essential_cookie_prefixes.is_empty()
    }

    /// The configured prefix that `name` belongs to, if any: equal to the
    /// prefix or starting with `<prefix>.` (case-insensitive).
    pub fn essential_family(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        self.essential_cookie_prefixes
            .iter()
            .find(|p| {
                lower == **p
                    || lower
                        .strip_prefix(p.as_str())
                        .is_some_and(|r| r.starts_with('.'))
            })
            .map(String::as_str)
    }

    /// Whether `name` is an essential cookie (name or prefix family).
    pub fn is_essential(&self, name: &str) -> bool {
        self.essential_cookie_names.contains(&name.to_lowercase())
            || self.essential_family(name).is_some()
    }

    /// Whether `name` is the preferred session cookie (case-insensitive).
    pub fn is_preferred_session_cookie(&self, name: &str) -> bool {
        name.to_lowercase() == self.preferred_session_cookie
    }
}

/// Events a session publishes to its pollers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthEvent {
    /// A *different* account signed in: clear the snapshot and the hourly and
    /// daily history.
    AccountReset,
    /// Explicit sign-out: clear the snapshot only.
    SignedOut,
}

/// Receives [`AuthEvent`]s.
pub trait AuthEventListener: Send + Sync {
    /// Called synchronously when the event fires.
    fn on_auth_event(&self, event: AuthEvent);
}

/// Monotonic counter bumped whenever the session is replaced or cleared, so an
/// in-flight request can tell its credentials are stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionGeneration(pub u64);

impl SessionGeneration {
    /// The next generation (saturating; wrap-around is not a concern at u64).
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Observable session state and its pure transitions.
///
/// Implementations of [`AuthSession`] embed this behind a lock and add I/O.
///
/// # Examples
///
/// ```
/// use tokenmon_core::AuthSessionState;
///
/// let mut s = AuthSessionState::default();
/// s.mark_signed_in(Some("a@b.com".into()));
/// assert!(!s.record_auth_failure("401"));
/// assert!(!s.record_auth_failure("401"));
/// assert!(s.record_auth_failure("401")); // third strike invalidates
/// assert!(s.needs_sign_in);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSessionState {
    /// A cookie header is stored.
    pub is_signed_in: bool,
    /// Account email, when known.
    pub account_email: Option<String>,
    /// Starts true; the UI shows the sign-in prompt.
    pub needs_sign_in: bool,
    /// Last capture or session failure reason.
    pub last_auth_error: Option<String>,
    /// Consecutive auth rejections since the last success.
    pub consecutive_auth_failures: u32,
    /// Bumped on save, refresh-from-disk and clear.
    pub session_generation: SessionGeneration,
}

impl Default for AuthSessionState {
    fn default() -> Self {
        Self {
            is_signed_in: false,
            account_email: None,
            needs_sign_in: true,
            last_auth_error: None,
            consecutive_auth_failures: 0,
            session_generation: SessionGeneration::default(),
        }
    }
}

impl AuthSessionState {
    /// `save(cookieHeader)` effect: signed in, failures reset, generation bumped.
    pub fn mark_signed_in(&mut self, email: Option<String>) {
        self.is_signed_in = true;
        self.needs_sign_in = false;
        self.account_email = email;
        self.last_auth_error = None;
        self.consecutive_auth_failures = 0;
        self.session_generation = self.session_generation.next();
    }

    /// `recordAuthFailure`: returns true when this was the
    /// [`AUTH_FAILURE_THRESHOLD`]th consecutive failure and the session was
    /// invalidated.
    pub fn record_auth_failure(&mut self, reason: &str) -> bool {
        self.consecutive_auth_failures += 1;
        if self.consecutive_auth_failures < AUTH_FAILURE_THRESHOLD {
            return false;
        }
        self.mark_session_invalid(Some(reason));
        true
    }

    /// `recordAuthSuccess`.
    pub fn record_auth_success(&mut self) {
        self.consecutive_auth_failures = 0;
    }

    /// `markSessionInvalid`: needs sign-in again, no `AccountReset`, the stored
    /// account identity is kept (so re-signing in as the same account is not a
    /// reset). Bumps the generation like `clearBrowserState`.
    pub fn mark_session_invalid(&mut self, reason: Option<&str>) {
        self.needs_sign_in = true;
        if let Some(r) = reason {
            self.last_auth_error = Some(r.to_owned());
        }
        self.is_signed_in = false;
        self.account_email = None;
        self.consecutive_auth_failures = 0;
        self.session_generation = self.session_generation.next();
    }

    /// `isCurrent(gen)`: the generation matches and the session is usable.
    pub fn is_current(&self, generation: SessionGeneration) -> bool {
        generation == self.session_generation && self.is_signed_in && !self.needs_sign_in
    }
}

/// What a poller needs from a provider cookie session.
///
/// Implemented by the real session (ws-webauth / ws-secrets) and by test fakes.
pub trait AuthSession: Send + Sync {
    /// Current `Cookie` header, after pruning to the essential allowlist.
    fn cookie_header(&self) -> Option<String>;

    /// Snapshot of the observable state.
    fn state(&self) -> AuthSessionState;

    /// Records an auth rejection; true when the session was invalidated.
    fn record_auth_failure(&self, reason: &str) -> bool;

    /// Records a successful authenticated request.
    fn record_auth_success(&self);

    /// Whether `generation` is still the live, signed-in session.
    fn is_current(&self, generation: SessionGeneration) -> bool;

    /// Registers a listener for [`AuthEvent`]s.
    fn add_listener(&self, listener: std::sync::Arc<dyn AuthEventListener>);
}
