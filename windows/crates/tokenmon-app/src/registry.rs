//! The provider registry: `(provider, poller, session)` in
//! [`ProviderId::USAGE_PROVIDERS`] order (spec 01 section 6.1 step 8).

use std::collections::BTreeMap;
use std::sync::Arc;

use tokenmon_core::ProviderId;

use crate::poller::SharedPoller;
use crate::sources::{HistoryAccess, HourlySource, NoHistory, ViewSource};

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
    views: BTreeMap<ProviderId, ViewSource>,
    hourly: BTreeMap<ProviderId, HourlySource>,
    history: Option<Arc<dyn HistoryAccess>>,
}

impl ProviderRegistry {
    /// Builds a registry, sorting `entries` into registry order.
    pub fn new(mut entries: Vec<ProviderEntry>) -> Self {
        entries.sort_by_key(|e| e.provider);
        Self {
            entries,
            views: BTreeMap::new(),
            hourly: BTreeMap::new(),
            history: None,
        }
    }

    /// Registers the Grok history access.
    #[must_use]
    pub fn with_history(mut self, history: Arc<dyn HistoryAccess>) -> Self {
        self.history = Some(history);
        self
    }

    /// The Grok history, or an empty one when none is registered.
    pub fn history(&self) -> Arc<dyn HistoryAccess> {
        self.history.clone().unwrap_or_else(|| Arc::new(NoHistory))
    }

    /// Registers `provider`'s typed panel source.
    #[must_use]
    pub fn with_view(mut self, provider: ProviderId, source: ViewSource) -> Self {
        self.views.insert(provider, source);
        self
    }

    /// Registers `provider`'s Overview hourly source.
    #[must_use]
    pub fn with_hourly(mut self, provider: ProviderId, source: HourlySource) -> Self {
        self.hourly.insert(provider, source);
        self
    }

    /// The panel source of `provider`, when registered.
    pub fn view(&self, provider: ProviderId) -> Option<&ViewSource> {
        self.views.get(&provider)
    }

    /// The Overview hourly source of `provider`, when registered.
    pub fn hourly(&self, provider: ProviderId) -> Option<&HourlySource> {
        self.hourly.get(&provider)
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
