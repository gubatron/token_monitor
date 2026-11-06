//! The concrete cookie session (spec 01 section 1.6.4 `ProviderAuthSession`)
//! and the companion [`ProviderAuthSession`] trait pollers code against.
//!
//! [`CookieAuthSession`] is generic over an `Arc<dyn CredentialStore>` and the
//! provider's [`AuthConfig`], so the production DPAPI-backed store and
//! [`crate::InMemoryCredentialStore`] drive identical logic.

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};

use crate::auth::{
    AUTH_FAILURE_THRESHOLD, AuthConfig, AuthEvent, AuthEventListener, AuthSession,
    AuthSessionState, CredentialKey, CredentialStore, CredentialStoreError, SessionGeneration,
};
use crate::clock::Clock;
use crate::net::split_folded_set_cookie;

/// What a finished sign-in capture hands the session (`CaptureResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedSession {
    /// The `Cookie` header to store.
    pub cookie_header: String,
    /// Account email found during capture, if any.
    pub email: Option<String>,
}

/// Clears browser-side state (WebView2 profile, shared cookie jar) on sign-out
/// and invalidation. Implemented by the platform layer.
pub trait BrowserStateCleaner: Send + Sync {
    /// Drops every cookie that belongs to the provider described by `config`.
    fn clear_browser_state(&self, config: &AuthConfig);
}

/// The session as pollers and sign-in UI drive it: core's read side
/// ([`AuthSession`]) plus every write of spec 1.6.4.
///
/// Replaces the per-provider `ClaudeSession`, `CursorSession` and
/// `RollingCookieSession` traits.
pub trait ProviderAuthSession: AuthSession {
    /// Sets or clears the "show the sign-in prompt" flag.
    fn set_needs_sign_in(&self, needs: bool);

    /// Remembers the account email. When the config has no identity cookie the
    /// email is the identity, so a different account fires
    /// [`AuthEvent::AccountReset`] synchronously (do not hold a lock the
    /// listener takes).
    fn save_account_email(&self, email: &str);

    /// Records the account identity; a change from a previously stored,
    /// different identity fires [`AuthEvent::AccountReset`].
    fn note_account_identity(&self, identity: &str);

    /// Folds renewed `Set-Cookie` values into the stored cookie header
    /// (essential names only; never bumps the session generation).
    fn apply_refreshed_cookies(&self, set_cookie_headers: &[String]);

    /// Needs sign-in again, deletes the stored session secrets, no reset event.
    fn mark_session_invalid(&self, reason: Option<&str>);

    /// Explicit sign-out: clears credentials and fires
    /// [`AuthEvent::SignedOut`].
    fn sign_out(&self);

    /// Stores a freshly captured cookie header and marks the session signed in.
    ///
    /// # Errors
    /// The store refused the write; the session is left signed out and
    /// `last_auth_error` says so.
    fn save(&self, cookie_header: &str) -> Result<(), CredentialStoreError>;

    /// Adopts a finished capture (cookie, identity, email).
    ///
    /// # Errors
    /// As [`ProviderAuthSession::save`].
    fn adopt(&self, captured: &CapturedSession) -> Result<(), CredentialStoreError>;

    /// Re-reads the store: signed-in flag, email, failures reset, generation
    /// bumped.
    fn refresh_from_disk(&self);

    /// The provider workspace remembered from the last good fetch (OpenCode),
    /// stored under [`CredentialKey::Workspace`]; empty counts as unset.
    fn workspace_id(&self) -> Option<String>;

    /// Remembers the workspace id (best effort: a refused write is logged only).
    fn set_workspace_id(&self, id: &str);

    /// The current session generation.
    fn session_generation(&self) -> SessionGeneration {
        self.state().session_generation
    }
}

/// Splits a cookie header into trimmed non-empty `(name, value)` pairs.
fn header_pairs(header: &str) -> Vec<(&str, &str)> {
    header
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .collect()
}

impl AuthConfig {
    /// `pruneCookieHeader`: narrows `header` to the essential cookies.
    ///
    /// Unchanged when there is no allowlist or no pair is essential (so a jar
    /// holding only a CSRF cookie is never narrowed to nothing).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::AuthConfig;
    /// # fn cfg() -> AuthConfig { AuthConfig {
    /// #   store_filename_prefix: "t_".into(), log_category: "T".into(), hosts: vec![],
    /// #   sign_out_hosts: vec![], extra_store_keys: vec![], preferred_session_cookie: "sso".into(),
    /// #   essential_cookie_names: vec!["sso".into()], essential_cookie_prefixes: vec![],
    /// #   account_identity_cookie: None, max_attempts: 4,
    /// #   retry_delay: AuthConfig::DEFAULT_RETRY_DELAY, failure_message: String::new() } }
    /// assert_eq!(cfg().prune_cookie_header("a=1; sso=2; b=3"), "sso=2");
    /// assert_eq!(cfg().prune_cookie_header("a=1; b=3"), "a=1; b=3");
    /// ```
    pub fn prune_cookie_header(&self, header: &str) -> String {
        if !self.has_essential_allowlist() {
            return header.to_owned();
        }
        let pairs = header_pairs(header);
        if !pairs.iter().any(|(n, _)| self.is_essential(n)) {
            return header.to_owned();
        }
        pairs
            .iter()
            .filter(|(n, _)| self.is_essential(n))
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// One parsed `Set-Cookie`: name, value and whether it is still alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshedCookie {
    /// Cookie name as sent.
    pub name: String,
    /// Cookie value (may be empty).
    pub value: String,
    /// Non-empty value and not expired (`Max-Age=0` or past `Expires` is dead).
    pub live: bool,
}

/// Parses every cookie in `set_cookie_headers` (comma-folded values included).
///
/// `Max-Age` wins over `Expires` as in RFC 6265; an unparsable `Expires` is
/// ignored. Total over arbitrary input.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::auth_session::parse_refreshed_cookies;
///
/// let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
/// let c = parse_refreshed_cookies(&["a=1; Path=/, b=; Max-Age=0".to_owned()], now);
/// assert!(c[0].live && !c[1].live);
/// ```
pub fn parse_refreshed_cookies(
    set_cookie_headers: &[String],
    now: DateTime<Utc>,
) -> Vec<RefreshedCookie> {
    set_cookie_headers
        .iter()
        .flat_map(|h| split_folded_set_cookie(h))
        .filter_map(|c| parse_one(&c, now))
        .collect()
}

fn parse_one(cookie: &str, now: DateTime<Utc>) -> Option<RefreshedCookie> {
    let mut parts = cookie.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let (name, value) = (name.trim(), value.trim());
    if name.is_empty() {
        return None;
    }
    let mut max_age: Option<i64> = None;
    let mut expires: Option<DateTime<Utc>> = None;
    for attr in parts {
        let Some((k, v)) = attr.split_once('=') else {
            continue;
        };
        match k.trim().to_ascii_lowercase().as_str() {
            "max-age" => max_age = v.trim().parse().ok(),
            "expires" => {
                expires = DateTime::parse_from_rfc2822(v.trim())
                    .ok()
                    .map(|d| d.with_timezone(&Utc));
            }
            _ => {}
        }
    }
    let unexpired = match (max_age, expires) {
        (Some(m), _) => m > 0,
        (None, Some(e)) => e > now,
        (None, None) => true,
    };
    Some(RefreshedCookie {
        name: name.to_owned(),
        value: value.to_owned(),
        live: unexpired && !value.is_empty(),
    })
}

/// Merges refreshed cookies into a stored header per spec 1.6.4 steps 2-6.
///
/// Returns the new header (possibly empty).
pub fn merge_refreshed_cookies(
    config: &AuthConfig,
    stored: &str,
    set_cookie_headers: &[String],
    now: DateTime<Utc>,
) -> String {
    let mut refreshed: Vec<RefreshedCookie> = Vec::new();
    for c in parse_refreshed_cookies(set_cookie_headers, now) {
        if !config.is_essential(&c.name) {
            continue;
        }
        match refreshed
            .iter_mut()
            .find(|r| r.name.eq_ignore_ascii_case(&c.name))
        {
            Some(slot) => *slot = c,
            None => refreshed.push(c),
        }
    }
    let renewed: Vec<&str> = refreshed
        .iter()
        .filter(|c| c.live)
        .filter_map(|c| config.essential_family(&c.name))
        .collect();
    let find = |name: &str| refreshed.iter().find(|r| r.name.eq_ignore_ascii_case(name));
    let mut out: Vec<String> = Vec::new();
    for (name, value) in header_pairs(stored) {
        match find(name) {
            Some(r) if r.live => out.push(format!("{}={}", r.name, r.value)),
            Some(_) => {}
            None if config
                .essential_family(name)
                .is_some_and(|f| renewed.contains(&f)) => {}
            None => out.push(format!("{name}={value}")),
        }
    }
    for r in refreshed.iter().filter(|r| r.live) {
        let present = header_pairs(stored)
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(&r.name));
        if !present {
            out.push(format!("{}={}", r.name, r.value));
        }
    }
    out.join("; ")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Concrete [`ProviderAuthSession`] over a [`CredentialStore`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use tokenmon_core::auth_session::{CookieAuthSession, ProviderAuthSession};
/// use tokenmon_core::{AuthConfig, AuthSession, InMemoryCredentialStore, SystemClock};
/// # fn cfg() -> AuthConfig { AuthConfig {
/// #   store_filename_prefix: "t_".into(), log_category: "T".into(), hosts: vec![],
/// #   sign_out_hosts: vec!["example.com".into()], extra_store_keys: vec![],
/// #   preferred_session_cookie: "sid".into(), essential_cookie_names: vec![],
/// #   essential_cookie_prefixes: vec![], account_identity_cookie: None, max_attempts: 4,
/// #   retry_delay: AuthConfig::DEFAULT_RETRY_DELAY, failure_message: String::new() } }
/// let s = CookieAuthSession::new(cfg(), Arc::new(InMemoryCredentialStore::default()), Arc::new(SystemClock));
/// s.save("sid=abc").unwrap();
/// assert!(s.state().is_signed_in);
/// assert_eq!(s.cookie_header().as_deref(), Some("sid=abc"));
/// ```
pub struct CookieAuthSession {
    config: AuthConfig,
    store: Arc<dyn CredentialStore>,
    clock: Arc<dyn Clock>,
    cleaner: Option<Arc<dyn BrowserStateCleaner>>,
    state: Mutex<AuthSessionState>,
    listeners: Mutex<Vec<Arc<dyn AuthEventListener>>>,
}

impl std::fmt::Debug for CookieAuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieAuthSession")
            .field("prefix", &self.config.store_filename_prefix)
            .field("state", &*lock(&self.state))
            .finish_non_exhaustive()
    }
}

impl CookieAuthSession {
    /// Opens the session and loads state from `store` (`init` +
    /// `refreshFromDisk`).
    pub fn new(config: AuthConfig, store: Arc<dyn CredentialStore>, clock: Arc<dyn Clock>) -> Self {
        let s = Self {
            config,
            store,
            clock,
            cleaner: None,
            state: Mutex::new(AuthSessionState::default()),
            listeners: Mutex::new(Vec::new()),
        };
        s.refresh_from_disk();
        s
    }

    /// Installs the browser-state cleaner used on sign-out and invalidation.
    #[must_use]
    pub fn with_browser_cleaner(mut self, cleaner: Arc<dyn BrowserStateCleaner>) -> Self {
        self.cleaner = Some(cleaner);
        self
    }

    /// The provider configuration.
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// `loadCookieHeader`: the stored cookie, pruned to the allowlist; a
    /// changed value is written back. Empty counts as absent.
    pub fn load_cookie_header(&self) -> Option<String> {
        let stored = self.store.get(CredentialKey::Session)?;
        let pruned = self.config.prune_cookie_header(&stored);
        if pruned != stored {
            // Best effort: the narrowed value is still returned if the write fails.
            let _ = self.store.set(CredentialKey::Session, &pruned);
        }
        Some(pruned).filter(|h| !h.is_empty())
    }

    fn fire(&self, event: Option<AuthEvent>) {
        let Some(event) = event else { return };
        let listeners: Vec<_> = lock(&self.listeners).clone();
        for l in listeners {
            l.on_auth_event(event);
        }
    }

    /// `clearBrowserState`: removes the session, email and extra keys and
    /// clears browser cookies. The account identity is kept.
    fn clear_credentials(&self) {
        self.store.remove(CredentialKey::Session);
        self.store.remove(CredentialKey::Email);
        for k in &self.config.extra_store_keys {
            self.store.remove(*k);
        }
        if let Some(c) = &self.cleaner {
            c.clear_browser_state(&self.config);
        }
    }

    /// Returns the event to fire when the identity changed.
    fn note_identity_locked(&self, id: &str) -> Option<AuthEvent> {
        let prev = self.store.get(CredentialKey::Account);
        if prev.as_deref() == Some(id) {
            return None;
        }
        let _ = self.store.set(CredentialKey::Account, id);
        prev.map(|_| AuthEvent::AccountReset)
    }

    fn identity_from(&self, captured: &CapturedSession) -> Option<String> {
        let cookie = self.config.account_identity_cookie.as_deref()?;
        header_pairs(&captured.cookie_header)
            .into_iter()
            .find(|(n, v)| n.eq_ignore_ascii_case(cookie) && !v.is_empty())
            .map(|(_, v)| v.to_owned())
    }

    fn fail_write(&self, e: &CredentialStoreError) {
        lock(&self.state).last_auth_error = Some(e.to_string());
    }
}

impl AuthSession for CookieAuthSession {
    fn cookie_header(&self) -> Option<String> {
        self.load_cookie_header()
    }

    fn state(&self) -> AuthSessionState {
        lock(&self.state).clone()
    }

    fn record_auth_failure(&self, reason: &str) -> bool {
        let n = {
            let mut s = lock(&self.state);
            s.consecutive_auth_failures += 1;
            s.consecutive_auth_failures
        };
        if n < AUTH_FAILURE_THRESHOLD {
            tracing::info!(
                category = %self.config.log_category,
                "auth rejection {n} of {AUTH_FAILURE_THRESHOLD}"
            );
            return false;
        }
        self.mark_session_invalid(Some(reason));
        true
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

impl ProviderAuthSession for CookieAuthSession {
    fn set_needs_sign_in(&self, needs: bool) {
        lock(&self.state).needs_sign_in = needs;
    }

    fn save_account_email(&self, email: &str) {
        let event = {
            let mut s = lock(&self.state);
            let _ = self.store.set(CredentialKey::Email, email);
            s.account_email = Some(email.to_owned());
            if self.config.account_identity_cookie.is_none() {
                self.note_identity_locked(email)
            } else {
                None
            }
        };
        self.fire(event);
    }

    fn note_account_identity(&self, identity: &str) {
        let event = {
            let _guard = lock(&self.state);
            self.note_identity_locked(identity)
        };
        self.fire(event);
    }

    fn apply_refreshed_cookies(&self, set_cookie_headers: &[String]) {
        if set_cookie_headers.is_empty() || self.config.sign_out_hosts.is_empty() {
            return;
        }
        let _guard = lock(&self.state);
        let Some(stored) = self.store.get(CredentialKey::Session) else {
            return;
        };
        let merged =
            merge_refreshed_cookies(&self.config, &stored, set_cookie_headers, self.clock.now());
        if merged != stored {
            // No generation bump: this is the same session, renewed.
            let _ = self.store.set(CredentialKey::Session, &merged);
        }
    }

    fn mark_session_invalid(&self, reason: Option<&str>) {
        let mut s = lock(&self.state);
        self.clear_credentials();
        s.mark_session_invalid(reason);
    }

    fn sign_out(&self) {
        {
            let mut s = lock(&self.state);
            self.clear_credentials();
            s.is_signed_in = false;
            s.account_email = None;
            s.needs_sign_in = true;
            s.last_auth_error = None;
            s.consecutive_auth_failures = 0;
            s.session_generation = s.session_generation.next();
        }
        self.fire(Some(AuthEvent::SignedOut));
    }

    fn save(&self, cookie_header: &str) -> Result<(), CredentialStoreError> {
        let mut s = lock(&self.state);
        if let Err(e) = self.store.set(CredentialKey::Session, cookie_header) {
            s.last_auth_error = Some(e.to_string());
            return Err(e);
        }
        s.is_signed_in = true;
        s.needs_sign_in = false;
        s.consecutive_auth_failures = 0;
        s.session_generation = s.session_generation.next();
        Ok(())
    }

    fn adopt(&self, captured: &CapturedSession) -> Result<(), CredentialStoreError> {
        if let Err(e) = self.save(&captured.cookie_header) {
            self.fail_write(&e);
            return Err(e);
        }
        let identity = self
            .identity_from(captured)
            .or_else(|| captured.email.clone());
        let event = {
            let mut s = lock(&self.state);
            let event = identity
                .as_deref()
                .and_then(|id| self.note_identity_locked(id));
            match &captured.email {
                Some(e) => {
                    let _ = self.store.set(CredentialKey::Email, e);
                }
                None => self.store.remove(CredentialKey::Email),
            }
            s.account_email.clone_from(&captured.email);
            s.is_signed_in = true;
            s.needs_sign_in = false;
            s.last_auth_error = None;
            event
        };
        self.fire(event);
        Ok(())
    }

    fn workspace_id(&self) -> Option<String> {
        self.store
            .get(CredentialKey::Workspace)
            .filter(|w| !w.is_empty())
    }

    fn set_workspace_id(&self, id: &str) {
        if let Err(e) = self.store.set(CredentialKey::Workspace, id) {
            tracing::warn!(category = %self.config.log_category, "could not store workspace id: {e}");
        }
    }

    fn refresh_from_disk(&self) {
        let signed_in = self.load_cookie_header().is_some();
        let email = self.store.get(CredentialKey::Email);
        let mut s = lock(&self.state);
        s.is_signed_in = signed_in;
        s.account_email = email;
        s.needs_sign_in = !signed_in;
        s.consecutive_auth_failures = 0;
        s.session_generation = s.session_generation.next();
    }
}
