//! Windows session-end messages (logoff, shutdown, restart), decoded without
//! touching the OS so the policy is testable (spec 01 section 6.8, review
//! #71).
//!
//! Neither winit nor Slint surfaces `WM_QUERYENDSESSION` / `WM_ENDSESSION`.
//! The shell owns a hidden top-level window whose window procedure feeds raw
//! messages through [`classify`] and runs the normal quit path when
//! [`SessionMessage::End`] says the session really is ending.

/// "May the session end?" (sent first, to every top-level window).
pub const WM_QUERYENDSESSION: u32 = 0x0011;

/// "The session is ending (or the end was cancelled)", sent after the query.
pub const WM_ENDSESSION: u32 = 0x0016;

/// A session-end message the shell must react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMessage {
    /// The session is about to end. The app never vetoes it.
    Query,
    /// The end was decided. `session_ending` is `false` when another program
    /// vetoed it and nothing will end after all.
    End {
        /// Whether the session is really ending.
        session_ending: bool,
    },
}

impl SessionMessage {
    /// The value the window procedure returns: `TRUE` (1) to let the session
    /// end for a query, `0` ("processed") for the end notification.
    pub const fn reply(self) -> isize {
        match self {
            Self::Query => 1,
            Self::End { .. } => 0,
        }
    }

    /// Whether the app must flush and quit now: only for an end that was not
    /// cancelled. Quitting on a cancelled end would close the app for nothing.
    pub const fn must_quit(self) -> bool {
        matches!(
            self,
            Self::End {
                session_ending: true
            }
        )
    }
}

/// Decodes a window message. `wparam` is the raw `WPARAM`.
///
/// ```
/// use tokenmon_app::session::{classify, SessionMessage, WM_ENDSESSION, WM_QUERYENDSESSION};
/// assert_eq!(classify(WM_QUERYENDSESSION, 0), Some(SessionMessage::Query));
/// assert!(classify(WM_ENDSESSION, 1).unwrap().must_quit());
/// assert!(!classify(WM_ENDSESSION, 0).unwrap().must_quit());
/// assert_eq!(classify(0x000F, 0), None);
/// ```
pub fn classify(message: u32, wparam: usize) -> Option<SessionMessage> {
    match message {
        WM_QUERYENDSESSION => Some(SessionMessage::Query),
        WM_ENDSESSION => Some(SessionMessage::End {
            session_ending: wparam != 0,
        }),
        _ => None,
    }
}
