//! Where system events come from, behind a trait with a portable fake.

use std::sync::{Arc, Mutex, PoisonError};

/// Receives system events. [`SystemWakeGate`](super::SystemWakeGate) implements it.
///
/// Methods may be called from any thread (the Windows message thread).
pub trait SystemEventSink: Send + Sync {
    /// The system is going to sleep.
    fn system_will_sleep(&self);
    /// The system woke up.
    fn system_did_wake(&self);
    /// Network reachability changed; `true` means internet is reachable.
    fn path_changed(&self, satisfied: bool);
}

/// Something that delivers system events to a sink once started.
pub trait SystemEventSource {
    /// Starts delivering events. The source keeps the sink alive until dropped.
    fn start(&self, sink: Arc<dyn SystemEventSink>);
}

/// In-memory source for tests: the test drives the events by hand.
#[derive(Default)]
pub struct FakeEventSource {
    sink: Mutex<Option<Arc<dyn SystemEventSink>>>,
}

impl FakeEventSource {
    /// A source with no sink yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn with_sink(&self, f: impl FnOnce(&dyn SystemEventSink)) {
        let sink = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sink) = sink {
            f(sink.as_ref());
        }
    }

    /// Emits a sleep.
    pub fn sleep(&self) {
        self.with_sink(|s| s.system_will_sleep());
    }

    /// Emits a wake.
    pub fn wake(&self) {
        self.with_sink(|s| s.system_did_wake());
    }

    /// Emits a network change.
    pub fn path(&self, satisfied: bool) {
        self.with_sink(|s| s.path_changed(satisfied));
    }
}

impl SystemEventSource for FakeEventSource {
    fn start(&self, sink: Arc<dyn SystemEventSink>) {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }
}
