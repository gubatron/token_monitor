//! The app model: lifecycle behaviour over the provider registry (spec 01
//! section 6).
//!
//! [`AppModel`] owns no UI. It is driven from the shell (menu open and close,
//! sign-in completed, power events, quit) and from the settings watch, and it
//! fans each event out to the registered pollers.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokenmon_core::ProviderId;
use tokenmon_core::settings::{AppSettings, Settings};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::registry::ProviderRegistry;

/// A poller refreshed on menu open must be at least this fresh.
pub const MENU_OPEN_FRESHNESS: Duration = Duration::from_secs(15);

/// Settings changes closer together than this are handled as one.
pub const CHANGE_COALESCING: Duration = Duration::from_millis(100);

type ShutdownHook = Box<dyn FnOnce() + Send>;

/// The settings that decide polling cadence: when any changes, every loop is
/// woken so it re-reads its interval.
#[derive(Debug, Clone, PartialEq)]
struct PollingInputs {
    needs_polling: Vec<bool>,
    active_seconds: u32,
    idle_seconds: u32,
}

impl PollingInputs {
    fn of(settings: &Settings) -> Self {
        Self {
            needs_polling: ProviderId::USAGE_PROVIDERS
                .iter()
                .map(|p| settings.needs_polling(*p))
                .collect(),
            active_seconds: settings.active_poll_seconds,
            idle_seconds: settings.idle_poll_seconds,
        }
    }
}

fn graph_providers(settings: &Settings) -> BTreeSet<ProviderId> {
    ProviderId::MENU_BAR_GRAPH_PROVIDERS
        .into_iter()
        .filter(|p| settings.shows_menu_bar_graph(*p))
        .collect()
}

/// Lifecycle owner: registry, settings and the runtime handle that runs
/// refreshes.
pub struct AppModel {
    registry: ProviderRegistry,
    settings: Arc<AppSettings>,
    handle: Handle,
    menu_open: AtomicBool,
    quit: AtomicBool,
    shutdown_hooks: Mutex<Vec<ShutdownHook>>,
}

impl AppModel {
    /// A model over `registry`; refreshes are spawned on `handle`.
    pub fn new(
        registry: ProviderRegistry,
        settings: Arc<AppSettings>,
        handle: Handle,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            settings,
            handle,
            menu_open: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            shutdown_hooks: Mutex::new(Vec::new()),
        })
    }

    /// The provider registry.
    pub fn registry(&self) -> &ProviderRegistry {
        &self.registry
    }

    /// The settings store.
    pub fn settings(&self) -> &Arc<AppSettings> {
        &self.settings
    }

    /// Whether the panel is currently open.
    pub fn is_menu_open(&self) -> bool {
        self.menu_open.load(Ordering::SeqCst)
    }

    /// Starts every loop: each refreshes now or parks until it is needed.
    pub fn start_all(&self) {
        for e in self.registry.entries() {
            e.poller.start();
        }
    }

    /// System sleep: pauses every loop and keeps its last refresh time.
    pub fn pause_all(&self) {
        for e in self.registry.entries() {
            e.poller.pause();
        }
    }

    /// The wake gate is ready (network back or 30 s passed): resumes every
    /// loop; an overdue loop refreshes at once.
    pub fn resume_all(&self) {
        for e in self.registry.entries() {
            e.poller.resume();
        }
    }

    /// Re-evaluates every loop's interval.
    pub fn wake_all(&self) {
        for e in self.registry.entries() {
            e.poller.wake();
        }
    }

    /// `setMenuOpen(isOpen)`: tells every poller (each wakes its loop when the
    /// value changes, switching active and idle cadence) and, on open, refreshes
    /// every provider that needs polling unless it is fresher than
    /// [`MENU_OPEN_FRESHNESS`].
    pub fn set_menu_open(&self, open: bool) {
        self.menu_open.store(open, Ordering::SeqCst);
        for e in self.registry.entries() {
            e.poller.set_menu_open(open);
        }
        if !open {
            return;
        }
        let settings = self.settings.snapshot();
        for e in self
            .registry
            .entries()
            .iter()
            .filter(|e| settings.needs_polling(e.provider))
        {
            let poller = Arc::clone(&e.poller);
            self.handle.spawn(async move {
                poller.refresh_now_if_older_than(MENU_OPEN_FRESHNESS).await;
            });
        }
    }

    /// Makes the menu-open state match whether a panel window is actually
    /// visible. A window can disappear without the shell's own hide path
    /// (Alt+F4, a close request), which would leave the pollers on the active
    /// cadence for good. Returns whether the state changed.
    pub fn sync_menu_open(&self, panel_visible: bool) -> bool {
        if self.is_menu_open() == panel_visible {
            return false;
        }
        self.set_menu_open(panel_visible);
        true
    }

    /// `signInCompleted(p)`: refreshes every poller sharing `p`'s session, so a
    /// Cursor sign-in refreshes Cursor and Grokbot.
    pub fn sign_in_completed(&self, provider: ProviderId) -> Vec<JoinHandle<()>> {
        self.registry
            .sharing_session_with(provider)
            .into_iter()
            .map(|e| {
                let poller = Arc::clone(&e.poller);
                self.handle.spawn(async move { poller.refresh_now().await })
            })
            .collect()
    }

    /// Refreshes every poller once and waits for all of them.
    pub async fn refresh_all(&self) {
        let tasks: Vec<_> = self
            .registry
            .entries()
            .iter()
            .map(|e| {
                let poller = Arc::clone(&e.poller);
                self.handle.spawn(async move { poller.refresh_now().await })
            })
            .collect();
        for t in tasks {
            // A panicking refresh is the poller's bug; the others still finish.
            let _ = t.await;
        }
    }

    /// Registers work to run once on [`quit`](Self::quit) (flushing stores).
    pub fn on_quit(&self, hook: impl FnOnce() + Send + 'static) {
        self.shutdown_hooks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(hook));
    }

    /// Whether [`quit`](Self::quit) already ran.
    pub fn has_quit(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    /// Stops every loop and runs the quit hooks, once.
    pub fn quit(&self) {
        if self.quit.swap(true, Ordering::SeqCst) {
            return;
        }
        for e in self.registry.entries() {
            e.poller.stop();
        }
        let hooks = std::mem::take(
            &mut *self
                .shutdown_hooks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for hook in hooks {
            hook();
        }
    }

    /// Watches the settings: a change to polling inputs wakes every loop; a
    /// provider newly added to the menu-bar graphs refreshes at once.
    ///
    /// Changes are coalesced over [`CHANGE_COALESCING`]. The task ends when the
    /// settings store is dropped.
    pub fn watch_settings(self: &Arc<Self>) -> JoinHandle<()> {
        let model = Arc::clone(self);
        let mut rx = self.settings.subscribe();
        // Read the starting value here, not in the task: a change between this
        // call and the task's first run must still count as a change.
        let initial = rx.borrow_and_update().clone();
        self.handle.spawn(async move {
            let mut polling = PollingInputs::of(&initial);
            let mut graphs = graph_providers(&initial);
            while rx.changed().await.is_ok() {
                tokio::time::sleep(CHANGE_COALESCING).await;
                let now = rx.borrow_and_update().clone();
                let next_polling = PollingInputs::of(&now);
                if next_polling != polling {
                    model.wake_all();
                    polling = next_polling;
                }
                let next_graphs = graph_providers(&now);
                for added in next_graphs.difference(&graphs) {
                    if let Some(e) = model.registry.entry(*added) {
                        let poller = Arc::clone(&e.poller);
                        tokio::spawn(async move { poller.refresh_now().await });
                    }
                }
                graphs = next_graphs;
            }
        })
    }
}
