//! The provider registry: `(provider, poller, session)` in
//! [`ProviderId::USAGE_PROVIDERS`] order (spec 01 section 6.1 step 8).

use std::sync::Arc;

use tokenmon_core::ProviderId;

use crate::poller::SharedPoller;

/// Identity of a session object. Two entries with the same key share one
/// session (Grokbot runs on Cursor's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionKey(usize);

impl SessionKey {
    /// The key of an `Arc`-held session: its allocation address.
    pub fn of<T: ?Sized>(session: &Arc<T>) -> Self {
        Self(Arc::as_ptr(session).cast::<()>() as usize)
    }
}

/// One registered provider.
#[derive(Clone)]
pub struct ProviderEntry {
    /// Which provider this is.
    pub provider: ProviderId,
    /// Its poller.
    pub poller: SharedPoller,
    /// The session it authenticates with.
    pub session: SessionKey,
}

/// All registered providers, in registry order.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    entries: Vec<ProviderEntry>,
}

impl ProviderRegistry {
    /// Builds a registry, sorting `entries` into registry order.
    pub fn new(mut entries: Vec<ProviderEntry>) -> Self {
        entries.sort_by_key(|e| e.provider);
        Self { entries }
    }

    /// Every entry, in [`ProviderId`] order.
    pub fn entries(&self) -> &[ProviderEntry] {
        &self.entries
    }

    /// The entry for `provider`.
    pub fn entry(&self, provider: ProviderId) -> Option<&ProviderEntry> {
        self.entries.iter().find(|e| e.provider == provider)
    }

    /// Every entry whose session is the same object as `provider`'s
    /// (`pollers(sharingSessionWith:)`).
    pub fn sharing_session_with(&self, provider: ProviderId) -> Vec<&ProviderEntry> {
        let Some(key) = self.entry(provider).map(|e| e.session) else {
            return Vec::new();
        };
        self.entries.iter().filter(|e| e.session == key).collect()
    }
}
