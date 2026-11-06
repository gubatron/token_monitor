//! Contract tests for the poller traits.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use chrono::{DateTime, Utc};
use proptest::prelude::*;
use tokenmon_core::{BoxFuture, ProviderError, ProviderId, ProviderUsagePoller, UsageFetcher};

#[derive(Default)]
struct Fake {
    open: AtomicBool,
    refreshes: AtomicU32,
}

impl ProviderUsagePoller for Fake {
    fn provider(&self) -> ProviderId {
        ProviderId::Claude
    }
    fn set_menu_open(&self, open: bool) {
        self.open.store(open, Ordering::SeqCst);
    }
    fn last_refreshed_at(&self) -> Option<DateTime<Utc>> {
        None
    }
    fn last_error(&self) -> Option<ProviderError> {
        None
    }
    fn headline_used_percent(&self) -> Option<f64> {
        Some(42.0)
    }
    fn headline_resets_at(&self) -> Option<DateTime<Utc>> {
        None
    }
    fn alert_account(&self) -> Option<String> {
        None
    }
    fn start(&self) {}
    fn stop(&self) {}
    fn refresh_now(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn clear_snapshot(&self) {}
}

impl UsageFetcher for Fake {
    type Snapshot = u32;
    #[allow(clippy::unused_async_trait_impl)]
    async fn fetch(&self) -> Result<u32, ProviderError> {
        Ok(7)
    }
    fn snapshot(&self) -> Option<u32> {
        Some(7)
    }
}

/// The poller trait is object-safe, so the app can hold a heterogeneous registry.
#[test]
fn test_poller_is_object_safe() {
    let p: Arc<dyn ProviderUsagePoller> = Arc::new(Fake::default());
    p.set_menu_open(true);
    assert_eq!(p.provider(), ProviderId::Claude);
    assert_eq!(p.headline_used_percent(), Some(42.0));
}

/// The typed fetcher half works with a native async fn and a snapshot type.
#[test]
fn test_fetcher_returns_snapshot() {
    let f = Fake::default();
    let fut = f.fetch();
    let mut fut = std::pin::pin!(fut);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    match fut.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(r) => assert_eq!(r.unwrap(), 7),
        std::task::Poll::Pending => panic!("ready future expected"),
    }
    assert_eq!(f.snapshot(), Some(7));
}

fn any_provider() -> impl Strategy<Value = ProviderId> {
    proptest::sample::select(ProviderId::ALL_CASES.to_vec())
}

proptest! {
    /// `normalized_order` always yields the 7 usage providers, no duplicates,
    /// no Overview, preserving the first-seen order of valid input.
    #[test]
    fn normalized_order_is_permutation(raw in proptest::collection::vec(any_provider(), 0..20)) {
        let out = ProviderId::normalized_order(&raw);
        let mut sorted = out.to_vec();
        sorted.sort();
        let mut expect = ProviderId::USAGE_PROVIDERS.to_vec();
        expect.sort();
        prop_assert_eq!(sorted, expect);
        let mut firsts: Vec<ProviderId> = vec![];
        for p in raw.iter().filter(|p| **p != ProviderId::Overview) {
            if !firsts.contains(p) { firsts.push(*p); }
        }
        prop_assert_eq!(&out[..firsts.len()], &firsts[..]);
    }
}
