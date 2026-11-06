//! The pure `SystemWakeGate` state machine (spec 01 section 1.7).

use std::time::Duration;

/// How long to wait for the network before resuming anyway.
pub const FALLBACK_DELAY: Duration = Duration::from_secs(30);

/// A side effect the owner of the machine must perform, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    /// Cancel the fallback timer.
    CancelFallback,
    /// Call `onSleep` (pause every polling loop).
    Sleep,
    /// (Re)start the fallback timer; when it fires, call [`GateMachine::fallback_fired`].
    StartFallback,
    /// Call `onReady` (resume every polling loop).
    Ready,
}

/// Gate state: `isNetworkSatisfied`, `isAwaitingNetwork`, `pathUpdatedSinceSleep`.
///
/// Pure and clock-free: every input returns the [`GateAction`]s to perform.
///
/// ```
/// use tokenmon_platform::power::{GateAction, GateMachine};
/// let mut g = GateMachine::new();
/// g.will_sleep();
/// assert_eq!(g.did_wake(), vec![GateAction::StartFallback]);
/// assert_eq!(g.path_changed(true), vec![GateAction::CancelFallback, GateAction::Ready]);
/// ```
#[derive(Debug, Clone)]
pub struct GateMachine {
    network_satisfied: bool,
    awaiting_network: bool,
    path_updated_since_sleep: bool,
}

impl Default for GateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl GateMachine {
    /// A machine awake with the network assumed satisfied.
    pub const fn new() -> Self {
        Self {
            network_satisfied: true,
            awaiting_network: false,
            path_updated_since_sleep: false,
        }
    }

    /// Whether a wake is waiting for the network.
    pub const fn is_awaiting_network(&self) -> bool {
        self.awaiting_network
    }

    /// `systemWillSleep()`.
    pub fn will_sleep(&mut self) -> Vec<GateAction> {
        self.awaiting_network = false;
        self.path_updated_since_sleep = false;
        vec![GateAction::CancelFallback, GateAction::Sleep]
    }

    /// `systemDidWake()`.
    pub fn did_wake(&mut self) -> Vec<GateAction> {
        self.awaiting_network = true;
        if self.path_updated_since_sleep && self.network_satisfied {
            self.finish_wake()
        } else {
            vec![GateAction::StartFallback]
        }
    }

    /// `pathChanged(satisfied)`. Does nothing outside a wake beyond recording.
    pub fn path_changed(&mut self, satisfied: bool) -> Vec<GateAction> {
        self.network_satisfied = satisfied;
        self.path_updated_since_sleep = true;
        if satisfied && self.awaiting_network {
            self.finish_wake()
        } else {
            Vec::new()
        }
    }

    /// The fallback timer fired: resume if still awaiting.
    pub fn fallback_fired(&mut self) -> Vec<GateAction> {
        self.finish_wake()
    }

    fn finish_wake(&mut self) -> Vec<GateAction> {
        if !self.awaiting_network {
            return Vec::new();
        }
        self.awaiting_network = false;
        vec![GateAction::CancelFallback, GateAction::Ready]
    }
}
