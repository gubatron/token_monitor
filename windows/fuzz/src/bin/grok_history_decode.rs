//! Invariants: loading an arbitrary history file never panics; when it cannot
//! be decoded the store reports `store_failed` and never writes the file back,
//! even after appends, a flush and a clear.
#![no_main]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::DateTime;
use libfuzzer_sys::fuzz_target;
use tokenmon_core::ManualClock;
use tokenmon_provider_grok::history::{BackingRead, GrokHistoryStore, HistoryBacking};
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;

struct Backing {
    text: String,
    writes: Arc<AtomicUsize>,
}

impl HistoryBacking for Backing {
    fn read(&self) -> BackingRead {
        BackingRead::Text(self.text.clone())
    }
    fn write(&self, _text: &str) -> bool {
        self.writes.fetch_add(1, Ordering::SeqCst);
        true
    }
}

fuzz_target!(|data: &[u8]| {
    let writes = Arc::new(AtomicUsize::new(0));
    let backing = Backing {
        text: String::from_utf8_lossy(data).into_owned(),
        writes: writes.clone(),
    };
    let clock = Arc::new(ManualClock::from_unix(1_800_000_000));
    let mut store = GrokHistoryStore::open(Some(Box::new(backing)), clock);
    let failed = store.store_failed();
    store.append(WeeklyUsageSnapshot::new(DateTime::UNIX_EPOCH, 10.0));
    store.flush();
    store.clear();
    if failed {
        assert_eq!(writes.load(Ordering::SeqCst), 0, "overwrote a bad file");
    }
});
