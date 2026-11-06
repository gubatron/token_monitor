//! Completion detection: the auth-host then return-page gate (spec 04 4.4) and
//! the navigation security policy.

use std::sync::Arc;

use url::Url;

/// `isAuthHost(host, path)`; the host is already lowercased.
pub type AuthHostFn = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
/// `isReturnPage(url)`.
pub type ReturnPageFn = Arc<dyn Fn(&Url) -> bool + Send + Sync>;

/// What a finished navigation means for sign-in completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEvent {
    /// Nothing of interest (`None`).
    None,
    /// First time an auth host was seen (`Auth host`).
    AuthHost,
    /// Back on the provider after authenticating (`Return page`).
    ReturnPage,
}

impl GateEvent {
    /// Display name.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::AuthHost => "Auth host",
            Self::ReturnPage => "Return page",
        }
    }
}

/// `SignInReturnGate`: the initial load of the start URL is ignored until an
/// auth host was seen.
#[derive(Clone)]
pub struct ReturnGate {
    is_auth_host: AuthHostFn,
    is_return_page: ReturnPageFn,
    did_see_auth: bool,
}

impl std::fmt::Debug for ReturnGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReturnGate")
            .field("did_see_auth", &self.did_see_auth)
            .finish_non_exhaustive()
    }
}

impl ReturnGate {
    /// A gate that has not seen an auth host yet.
    pub fn new(is_auth_host: AuthHostFn, is_return_page: ReturnPageFn) -> Self {
        Self {
            is_auth_host,
            is_return_page,
            did_see_auth: false,
        }
    }

    /// Whether an auth host has been seen.
    pub fn did_see_auth(&self) -> bool {
        self.did_see_auth
    }

    /// `note(url)`, called on every finished navigation (main or popup).
    /// A URL that is both auth host and return page reports `ReturnPage`.
    pub fn note(&mut self, url: &Url) -> GateEvent {
        let host = url.host_str().unwrap_or("").to_lowercase();
        let mut event = GateEvent::None;
        if !self.did_see_auth && (self.is_auth_host)(&host, url.path()) {
            self.did_see_auth = true;
            event = GateEvent::AuthHost;
        }
        if self.did_see_auth && (self.is_return_page)(url) {
            return GateEvent::ReturnPage;
        }
        event
    }

    /// `matchesReturnPage(url)`.
    pub fn matches_return_page(&self, url: &Url) -> bool {
        self.did_see_auth && (self.is_return_page)(url)
    }
}

/// Whether the sign-in browser may navigate to `uri`: `http`/`https` with a
/// host, or exactly `about:blank` (the initial state of a popup). Everything
/// else (`file:`, `data:`, `javascript:`, `ms-*:`, `blob:` ...) is refused so
/// a provider page can never steer the window to local or app-internal
/// resources.
pub fn is_navigation_allowed(uri: &str) -> bool {
    if uri == "about:blank" {
        return true;
    }
    match Url::parse(uri) {
        Ok(u) => {
            matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty())
        }
        Err(_) => false,
    }
}
