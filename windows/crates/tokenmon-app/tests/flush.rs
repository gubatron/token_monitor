//! The Grok history reaches the disk: debounced while running and on every
//! quit path (#64).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use tokenmon_app::flush::{FLUSH_DEBOUNCE, flush_on_quit, spawn_history_flusher};
use tokenmon_app::model::AppModel;
use tokenmon_app::registry::ProviderRegistry;
use tokenmon_app::sources::HistoryAccess;
use tokenmon_core::settings::{AppSettings, SettingsFile};
use tokenmon_core::{Clock, SystemClock};
use tokenmon_provider_grok::history::{FileBacking, GrokHistoryStore};
use tokenmon_provider_grok::models::WeeklyUsageSnapshot;

/// A real store on a real file behind the same trait the app uses.
struct StoreHistory(Mutex<GrokHistoryStore>);

impl StoreHistory {
    fn open(dir: &Path) -> Arc<Self> {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Arc::new(Self(Mutex::new(GrokHistoryStore::open(
            Some(Box::new(FileBacking::in_dir(dir))),
            clock,
        ))))
    }
    fn append(&self, percent: f64) {
        let at = Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .append(WeeklyUsageSnapshot::new(at, percent));
    }
}

impl HistoryAccess for StoreHistory {
    fn snapshots(&self) -> Vec<WeeklyUsageSnapshot> {
        let g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        g.all_snapshots().to_vec()
    }
    fn clear(&self) {}
    fn store_failed(&self) -> bool {
        false
    }
    fn is_dirty(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_dirty()
    }
    fn flush(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush();
    }
}

fn model() -> Arc<AppModel> {
    AppModel::new(
        ProviderRegistry::new(Vec::new()),
        Arc::new(AppSettings::new(Arc::new(SettingsFile::in_memory()))),
        tokio::runtime::Handle::current(),
    )
}

fn reload(dir: &Path) -> usize {
    StoreHistory::open(dir).snapshots().len()
}

/// Appended rows are lost without a flush: quitting must write them.
#[tokio::test]
async fn test_quit_flushes_appended_history_to_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let history = StoreHistory::open(dir.path());
    let model = model();
    flush_on_quit(&model, history.clone());
    history.append(40.0);
    assert_eq!(reload(dir.path()), 0, "nothing written before quit");
    model.quit();
    assert_eq!(reload(dir.path()), 1);
}

/// Quit from several paths at once (tray, Settings, session end) runs the
/// flush hook exactly once.
#[tokio::test]
async fn test_concurrent_quits_run_hooks_once() {
    let model = model();
    let runs = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&runs);
    model.on_quit(move || {
        r.fetch_add(1, Ordering::SeqCst);
    });
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let m = Arc::clone(&model);
            std::thread::spawn(move || m.quit())
        })
        .collect();
    for t in threads {
        t.join().expect("quit thread");
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

/// The debounced flusher writes a dirty history shortly after the append and
/// stays quiet while it is clean.
#[tokio::test(start_paused = true)]
async fn test_debounced_flusher_writes_dirty_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let history = StoreHistory::open(dir.path());
    let task = spawn_history_flusher(&tokio::runtime::Handle::current(), history.clone());
    history.append(10.0);
    assert!(history.is_dirty());
    tokio::time::sleep(FLUSH_DEBOUNCE * 2 + Duration::from_millis(50)).await;
    assert!(!history.is_dirty());
    assert_eq!(reload(dir.path()), 1);
    task.abort();
}
