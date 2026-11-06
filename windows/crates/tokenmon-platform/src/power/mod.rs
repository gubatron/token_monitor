//! Sleep/wake (`WM_POWERBROADCAST`), network reachability and `SystemWakeGate`.
//!
//! Owner: ws-power (spec 01 section 1.7 and 6.5).
//!
//! The gate pauses every polling loop across system sleep and resumes them
//! once the network is back, or after a 30 s fallback.
//!
//! * [`machine`]: the pure gate state machine ([`GateMachine`]), no clock, no
//!   tasks; returns [`GateAction`]s.
//! * [`gate`]: [`SystemWakeGate`], the machine plus the injectable fallback
//!   timer and the `on_sleep` / `on_ready` callbacks.
//! * [`event`]: [`PowerEvent`], the `WM_POWERBROADCAST` decoder and the
//!   [`ResumeDeduper`] that turns the several resume messages into one wake.
//! * [`source`]: [`SystemEventSource`] and the [`FakeEventSource`] used by tests.
//! * `windows` (cfg(windows)): the message-only window and the connectivity-hint
//!   subscription that feed a [`SystemEventSink`].

pub mod event;
pub mod gate;
pub mod machine;
pub mod source;
#[cfg(windows)]
pub mod windows;

pub use event::{NetworkLevel, PowerEvent, ResumeDeduper, decode_power_broadcast};
pub use gate::{GateCallbacks, SystemWakeGate};
pub use machine::{FALLBACK_DELAY, GateAction, GateMachine};
pub use source::{FakeEventSource, SystemEventSink, SystemEventSource};
