//! Decoding of `WM_POWERBROADCAST` and connectivity levels into gate events.

use std::time::{Duration, Instant};

/// `PBT_APMSUSPEND`: the system is about to suspend.
pub const PBT_APMSUSPEND: u32 = 0x0004;
/// `PBT_APMRESUMESUSPEND`: resumed after a user-visible suspend.
pub const PBT_APMRESUMESUSPEND: u32 = 0x0007;
/// `PBT_APMRESUMEAUTOMATIC`: resumed with no user present (also Modern Standby).
pub const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;

/// A power transition relevant to polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerEvent {
    /// The system is going to sleep (`systemWillSleep`).
    WillSleep,
    /// The system woke up (`systemDidWake`).
    DidWake,
}

/// Maps a `WM_POWERBROADCAST` `wParam` to a raw event; unknown codes give `None`.
///
/// Both resume codes map to [`PowerEvent::DidWake`]; use [`ResumeDeduper`] to
/// collapse the pair Windows often sends into one.
///
/// ```
/// use tokenmon_platform::power::{decode_power_broadcast, PowerEvent};
/// assert_eq!(decode_power_broadcast(0x4), Some(PowerEvent::WillSleep));
/// assert_eq!(decode_power_broadcast(0x12), Some(PowerEvent::DidWake));
/// assert_eq!(decode_power_broadcast(0xA), None);
/// ```
pub const fn decode_power_broadcast(wparam: u32) -> Option<PowerEvent> {
    match wparam {
        PBT_APMSUSPEND => Some(PowerEvent::WillSleep),
        PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => Some(PowerEvent::DidWake),
        _ => None,
    }
}

/// Resume messages closer together than this are one wake (Windows sends
/// `PBT_APMRESUMEAUTOMATIC` and `PBT_APMRESUMESUSPEND` within milliseconds, and
/// registering for suspend/resume notifications can repeat them).
pub const RESUME_DEDUPE_WINDOW: Duration = Duration::from_secs(5);

/// Collapses the resume messages of one wake into a single
/// [`PowerEvent::DidWake`].
///
/// A wake passes unless another wake passed within [`RESUME_DEDUPE_WINDOW`]; a
/// suspend always passes and forgets the last wake, so every suspend to resume
/// pair yields one wake. Dedupe is by time, not "until the next suspend",
/// because Modern Standby can resume repeatedly without any suspend message.
///
/// ```
/// use std::time::{Duration, Instant};
/// use tokenmon_platform::power::{PowerEvent, ResumeDeduper};
/// let t0 = Instant::now();
/// let mut d = ResumeDeduper::default();
/// assert_eq!(d.accept_at(0x12, t0), Some(PowerEvent::DidWake));
/// assert_eq!(d.accept_at(0x07, t0 + Duration::from_millis(20)), None);
/// // A later Modern Standby resume with no suspend message still wakes.
/// assert_eq!(d.accept_at(0x12, t0 + Duration::from_secs(600)), Some(PowerEvent::DidWake));
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ResumeDeduper {
    last_wake: Option<Instant>,
}

impl ResumeDeduper {
    /// Decodes `wparam` observed now and returns the event to forward, if any.
    pub fn accept(&mut self, wparam: u32) -> Option<PowerEvent> {
        self.accept_at(wparam, Instant::now())
    }

    /// As [`accept`](Self::accept) with an explicit observation time, so the
    /// window logic is testable without sleeping.
    ///
    /// Mutation is the point of a dedupe filter: it is one timestamp of state.
    pub fn accept_at(&mut self, wparam: u32, now: Instant) -> Option<PowerEvent> {
        match decode_power_broadcast(wparam)? {
            PowerEvent::WillSleep => {
                self.last_wake = None;
                Some(PowerEvent::WillSleep)
            }
            PowerEvent::DidWake => {
                let repeat = self
                    .last_wake
                    .is_some_and(|last| now.saturating_duration_since(last) < RESUME_DEDUPE_WINDOW);
                if repeat {
                    None
                } else {
                    self.last_wake = Some(now);
                    Some(PowerEvent::DidWake)
                }
            }
        }
    }
}

/// How much connectivity the network stack reports.
///
/// Mirrors `NL_NETWORK_CONNECTIVITY_LEVEL_HINT`. Only unconstrained internet
/// access counts as "satisfied" (the Network List Manager's IPv4 or IPv6
/// internet flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkLevel {
    /// Unknown.
    Unknown,
    /// No connectivity.
    None,
    /// Local network only.
    LocalAccess,
    /// Internet access.
    InternetAccess,
    /// A captive portal intercepts traffic.
    ConstrainedInternetAccess,
    /// Hidden (not reported by the stack).
    Hidden,
}

impl NetworkLevel {
    /// Converts the raw `NL_NETWORK_CONNECTIVITY_LEVEL_HINT` value.
    pub const fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::None,
            2 => Self::LocalAccess,
            3 => Self::InternetAccess,
            4 => Self::ConstrainedInternetAccess,
            5 => Self::Hidden,
            _ => Self::Unknown,
        }
    }

    /// True when the path is usable for provider requests.
    pub const fn is_satisfied(self) -> bool {
        matches!(self, Self::InternetAccess)
    }
}

/// `NLM_CONNECTIVITY_IPV4_INTERNET`.
pub const NLM_IPV4_INTERNET: u32 = 0x0040;
/// `NLM_CONNECTIVITY_IPV6_INTERNET`.
pub const NLM_IPV6_INTERNET: u32 = 0x0400;

/// Whether Network List Manager connectivity flags mean "satisfied".
///
/// ```
/// use tokenmon_platform::power::event::network_flags_satisfied;
/// assert!(network_flags_satisfied(0x40));
/// assert!(network_flags_satisfied(0x400 | 0x1));
/// assert!(!network_flags_satisfied(0x10)); // IPv4 local network only
/// ```
pub const fn network_flags_satisfied(flags: u32) -> bool {
    flags & (NLM_IPV4_INTERNET | NLM_IPV6_INTERNET) != 0
}
