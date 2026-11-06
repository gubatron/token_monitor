//! Decoding of `WM_POWERBROADCAST` and connectivity levels into gate events.

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

/// Collapses repeated resume messages: one [`PowerEvent::DidWake`] per sleep.
///
/// Windows sends `PBT_APMRESUMEAUTOMATIC` and `PBT_APMRESUMESUSPEND` together.
/// A wake is passed once; further wakes are dropped until the next sleep. A
/// wake with no preceding sleep (Modern Standby without a suspend message)
/// still passes the first time.
///
/// ```
/// use tokenmon_platform::power::{PowerEvent, ResumeDeduper};
/// let mut d = ResumeDeduper::default();
/// assert_eq!(d.accept(0x12), Some(PowerEvent::DidWake));
/// assert_eq!(d.accept(0x07), None);
/// assert_eq!(d.accept(0x04), Some(PowerEvent::WillSleep));
/// assert_eq!(d.accept(0x07), Some(PowerEvent::DidWake));
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct ResumeDeduper {
    woke_since_sleep: bool,
}

impl ResumeDeduper {
    /// Decodes `wparam` and returns the event to forward, if any.
    ///
    /// Mutation is the point of a dedupe filter: it is one bool of state.
    pub fn accept(&mut self, wparam: u32) -> Option<PowerEvent> {
        match decode_power_broadcast(wparam)? {
            PowerEvent::WillSleep => {
                self.woke_since_sleep = false;
                Some(PowerEvent::WillSleep)
            }
            PowerEvent::DidWake if self.woke_since_sleep => None,
            PowerEvent::DidWake => {
                self.woke_since_sleep = true;
                Some(PowerEvent::DidWake)
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
