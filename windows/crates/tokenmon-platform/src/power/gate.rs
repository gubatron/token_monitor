//! [`SystemWakeGate`]: pause polling across sleep, resume when the network is back.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokenmon_core::polling::Sleeper;
use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use super::machine::{FALLBACK_DELAY, GateAction, GateMachine};
use super::source::SystemEventSink;

type Callback = Arc<dyn Fn() + Send + Sync>;

/// The two outputs of the gate.
#[derive(Clone)]
pub struct GateCallbacks {
    /// Called on sleep: the app pauses every `PollingLoop`.
    pub on_sleep: Callback,
    /// Called when the network is back (or after 30 s): the app resumes every loop.
    pub on_ready: Callback,
}

struct Shared {
    state: Mutex<State>,
    sleeper: Arc<dyn Sleeper>,
    handle: Handle,
    callbacks: GateCallbacks,
}

struct State {
    machine: GateMachine,
    /// Bumped on every (re)start or cancel so a stale timer cannot finish a newer wake.
    epoch: u64,
    fallback: Option<AbortHandle>,
}

/// Pauses polling on sleep and resumes it when the network returns or 30 s pass.
///
/// Create one only in the real app. Feed it through a
/// [`SystemEventSource`](super::SystemEventSource) or call the
/// [`SystemEventSink`] methods directly. The fallback wait uses the injected
/// [`Sleeper`], so tests drive it with `ManualScheduler`. Dropping the gate
/// cancels the fallback.
pub struct SystemWakeGate {
    shared: Arc<Shared>,
}

impl SystemWakeGate {
    /// Builds a gate whose fallback timer runs on `handle` using `sleeper`.
    pub fn new(sleeper: Arc<dyn Sleeper>, handle: Handle, callbacks: GateCallbacks) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    machine: GateMachine::new(),
                    epoch: 0,
                    fallback: None,
                }),
                sleeper,
                handle,
                callbacks,
            }),
        }
    }

    /// Whether a wake is currently waiting for the network.
    pub fn is_awaiting_network(&self) -> bool {
        self.shared.lock().machine.is_awaiting_network()
    }
}

impl Drop for SystemWakeGate {
    fn drop(&mut self) {
        if let Some(task) = self.shared.lock().fallback.take() {
            task.abort();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs one machine step and performs its actions. Callbacks run after the
    /// lock is released so they may re-enter the gate.
    fn step(self: &Arc<Self>, run: impl FnOnce(&mut State) -> Vec<GateAction>) {
        let mut callbacks: Vec<Callback> = Vec::new();
        {
            let mut state = self.lock();
            for action in run(&mut state) {
                match action {
                    GateAction::CancelFallback => {
                        state.epoch += 1;
                        if let Some(task) = state.fallback.take() {
                            task.abort();
                        }
                    }
                    GateAction::StartFallback => self.start_fallback(&mut state),
                    GateAction::Sleep => callbacks.push(Arc::clone(&self.callbacks.on_sleep)),
                    GateAction::Ready => callbacks.push(Arc::clone(&self.callbacks.on_ready)),
                }
            }
        }
        for callback in &callbacks {
            callback();
        }
    }

    fn start_fallback(self: &Arc<Self>, state: &mut State) {
        state.epoch += 1;
        if let Some(old) = state.fallback.take() {
            old.abort();
        }
        let epoch = state.epoch;
        let shared = Arc::clone(self);
        let wait = self.sleeper.sleep(FALLBACK_DELAY);
        let task = self.handle.spawn(async move {
            wait.await;
            shared.step(|state| {
                if state.epoch == epoch {
                    state.machine.fallback_fired()
                } else {
                    Vec::new()
                }
            });
        });
        state.fallback = Some(task.abort_handle());
    }
}

impl SystemEventSink for SystemWakeGate {
    fn system_will_sleep(&self) {
        self.shared.step(|s| s.machine.will_sleep());
    }

    fn system_did_wake(&self) {
        self.shared.step(|s| s.machine.did_wake());
    }

    fn path_changed(&self, satisfied: bool) {
        self.shared.step(|s| s.machine.path_changed(satisfied));
    }
}
