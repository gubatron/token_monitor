//! The sign-in state machine (spec 04 sections 4.2 to 4.4).
//!
//! A pure, synchronous reducer: browser events and timer expiries go in as
//! [`Input`]s, and [`Action`]s (start a timer, read cookies, close the window,
//! ...) come out. It never sleeps, touches the OS or reads a clock, so every
//! timing in the spec is tested by feeding timer inputs directly. The
//! WebView2 host is a thin adapter that executes the actions.

use std::sync::Arc;
use std::time::Duration;

use url::Url;

use super::config::SignInConfig;
use super::cookies::{self, CapturePolicy, CapturedCookies, WebCookie};
use super::gate::{GateEvent, ReturnGate};
use super::nav::{BackAction, NavSnapshot, NavState};

/// Status hint appears after this long without completion.
pub const STATUS_HINT_DELAY: Duration = Duration::from_secs(25);
/// Poll interval while waiting for a popup to close before capturing.
pub const POPUP_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Maximum popup polls (200 ms x 40 = 8 s).
pub const POPUP_POLL_MAX: u32 = 40;
/// How long "Signed in" stays visible before the window closes.
pub const CONFIRM_DISMISS_DELAY: Duration = Duration::from_millis(1100);
/// Wait before the single automatic capture retry.
pub const AUTO_RETRY_DELAY: Duration = Duration::from_millis(1200);

/// Which web view a navigation happened in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// The main browser view.
    Main,
    /// An OAuth popup.
    Popup,
}

/// Timers the machine asks the host to run (one-shot, restartable by id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimerId {
    /// 25 s "Signed in? Choose Finish Sign-In" hint.
    StatusHint,
    /// 200 ms popup poll before the return delay.
    PopupPoll,
    /// The provider's return delay.
    ReturnDelay,
    /// Delay between cookie read attempts.
    CaptureAttempt,
    /// 1.2 s before the automatic retry of a failed capture.
    AutoRetry,
    /// 1.1 s before closing after success.
    ConfirmDismiss,
}

/// Sign-in phase (`signingIn`, `finishing`, `signedIn`, `failed`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for the user.
    SigningIn,
    /// Capturing the session.
    Finishing,
    /// Captured; the window is about to close.
    SignedIn {
        /// Account e-mail, when known.
        email: Option<String>,
    },
    /// Capture failed; the user can retry.
    Failed {
        /// Message to show.
        message: String,
    },
}

/// Events from the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The window is on screen.
    Opened,
    /// A navigation finished in a view.
    NavigationFinished {
        /// Which view.
        view: ViewKind,
        /// Final URL.
        url: String,
    },
    /// The active view's history/loading state changed.
    NavSnapshot(NavSnapshot),
    /// A popup view was created and shown.
    PopupOpened,
    /// A popup was dismissed (user, Close popup or `window.close()`).
    PopupClosed,
    /// Back button.
    BackClicked,
    /// "Close popup" button.
    ClosePopupClicked,
    /// "Finish Sign-In" / "Try Again".
    FinishClicked,
    /// "Cancel" / Escape.
    CancelClicked,
    /// The window was closed by any means.
    WindowClosed,
    /// A timer fired.
    TimerFired(TimerId),
    /// Result of [`Action::ReadCookies`].
    CookiesRead(Vec<WebCookie>),
    /// Result of [`Action::Adopt`]: the app saved the session (and ran the
    /// provider's after-capture step). `Ok` carries the account e-mail.
    AdoptFinished(Result<Option<String>, String>),
}

/// Effects for the host to perform.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// (Re)start a one-shot timer.
    StartTimer {
        /// Timer.
        id: TimerId,
        /// Delay.
        delay: Duration,
    },
    /// Cancel a timer if running.
    CancelTimer(TimerId),
    /// Navigate the active view back.
    GoBack,
    /// Dismiss the top popup (then report [`Input::PopupClosed`]).
    ClosePopup,
    /// Read every cookie of the sign-in profile, report [`Input::CookiesRead`].
    ReadCookies,
    /// Hand the captured session to the app, report [`Input::AdoptFinished`].
    Adopt(CapturedCookies),
    /// Provider hook `onReturned(url)`: the return page was detected.
    OnReturned(String),
    /// Provider hook `onComplete`: sign-in succeeded.
    NotifyComplete,
    /// Close the window.
    CloseWindow,
    /// The view model changed; re-render.
    UpdateView(ViewModel),
}

/// Everything the chrome (header, navigation bar, footer) displays.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per control state
pub struct ViewModel {
    /// Window/header title.
    pub title: String,
    /// Header status text.
    pub status_text: String,
    /// Green check next to the title (signed-in phase).
    pub show_check: bool,
    /// Back button enabled.
    pub back_enabled: bool,
    /// Back button tooltip.
    pub back_tooltip: &'static str,
    /// Forward button enabled.
    pub forward_enabled: bool,
    /// Reload button shows "Stop".
    pub is_loading: bool,
    /// Address label.
    pub address: String,
    /// The current URL (address tooltip).
    pub full_url: String,
    /// "Close popup" button visible.
    pub show_close_popup: bool,
    /// Footer spinner visible.
    pub finishing_spinner: bool,
    /// Primary button title.
    pub primary_title: &'static str,
    /// Primary button enabled.
    pub primary_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Idle,
    /// Waiting for popups to close; `polls` already done.
    PopupWait {
        polls: u32,
    },
    /// Return delay running.
    ReturnDelay,
    /// Reading cookies, `attempt` is 1-based.
    Reading {
        attempt: u32,
        automatic: bool,
    },
    /// Adopt dispatched.
    Adopting {
        automatic: bool,
    },
    /// Waiting to retry an automatic capture.
    AutoRetryWait,
}

/// The reducer. See the module docs.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // independent latches from the spec
pub struct SignInMachine {
    title: String,
    subtitle: String,
    return_delay: Duration,
    policy: CapturePolicy,
    gate: ReturnGate,
    phase: Phase,
    flow: Flow,
    nav: NavState,
    hint_shown: bool,
    dismissed: bool,
    main_url: Option<Url>,
    did_navigate_main_after_auth: bool,
    did_fire_return: bool,
    auto_retry_used: bool,
    last_view: Option<ViewModel>,
}

impl SignInMachine {
    /// A machine for one sign-in session.
    pub fn new(config: &SignInConfig) -> Self {
        Self {
            title: config.title.clone(),
            subtitle: config.subtitle.clone(),
            return_delay: config.return_delay,
            policy: config.policy.clone(),
            gate: ReturnGate::new(
                Arc::clone(&config.is_auth_host),
                Arc::clone(&config.is_return_page),
            ),
            phase: Phase::SigningIn,
            flow: Flow::Idle,
            nav: NavState::default(),
            hint_shown: false,
            dismissed: false,
            main_url: None,
            did_navigate_main_after_auth: false,
            did_fire_return: false,
            auto_retry_used: false,
            last_view: None,
        }
    }

    /// Current phase.
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Open popups.
    pub fn popup_depth(&self) -> usize {
        self.nav.popup_depth
    }

    /// Whether the window was dismissed.
    pub fn is_dismissed(&self) -> bool {
        self.dismissed
    }

    /// Current navigation state.
    pub fn nav(&self) -> &NavState {
        &self.nav
    }

    /// The header status text for the current state.
    pub fn status_text(&self) -> String {
        match &self.phase {
            Phase::SigningIn => {
                if self.nav.has_popup() {
                    "Complete sign-in in the popup above.".to_owned()
                } else if self.hint_shown {
                    "Signed in? Choose Finish Sign-In to complete.".to_owned()
                } else {
                    self.subtitle.clone()
                }
            }
            Phase::Finishing => "Finishing sign-in\u{2026}".to_owned(),
            Phase::SignedIn { email: Some(e) } => format!("Signed in as {e}"),
            Phase::SignedIn { email: None } => "Signed in".to_owned(),
            Phase::Failed { message } => message.clone(),
        }
    }

    /// The view model for the current state.
    pub fn view_model(&self) -> ViewModel {
        let (primary_title, primary_enabled) = match self.phase {
            Phase::SigningIn => ("Finish Sign-In", true),
            Phase::Finishing => ("Finishing\u{2026}", false),
            Phase::Failed { .. } => ("Try Again", true),
            Phase::SignedIn { .. } => ("Signed In", false),
        };
        ViewModel {
            title: self.title.clone(),
            status_text: self.status_text(),
            show_check: matches!(self.phase, Phase::SignedIn { .. }),
            back_enabled: self.nav.back_is_enabled(),
            back_tooltip: self.nav.back_tooltip(),
            forward_enabled: self.nav.snapshot.can_go_forward,
            is_loading: self.nav.snapshot.is_loading,
            address: self.nav.address_label(),
            full_url: self.nav.snapshot.current_url.clone().unwrap_or_default(),
            show_close_popup: self.nav.has_popup(),
            finishing_spinner: matches!(self.phase, Phase::Finishing),
            primary_title,
            primary_enabled,
        }
    }

    /// Advances the machine by one input and returns the effects.
    pub fn handle(&mut self, input: Input) -> Vec<Action> {
        let mut out = Vec::new();
        if self.dismissed && !matches!(input, Input::WindowClosed) {
            return out;
        }
        match input {
            Input::Opened => out.push(Action::StartTimer {
                id: TimerId::StatusHint,
                delay: STATUS_HINT_DELAY,
            }),
            Input::NavigationFinished { view, url } => {
                self.navigation_finished(view, &url, &mut out);
            }
            Input::NavSnapshot(s) => self.nav.snapshot = s,
            Input::PopupOpened => self.nav.popup_depth += 1,
            Input::PopupClosed => self.popup_closed(&mut out),
            Input::BackClicked => {
                match BackAction::decide(self.nav.snapshot.can_go_back, self.nav.popup_depth) {
                    BackAction::GoBack => out.push(Action::GoBack),
                    BackAction::ClosePopup => out.push(Action::ClosePopup),
                    BackAction::None => {}
                }
            }
            Input::ClosePopupClicked => {
                if self.nav.has_popup() {
                    out.push(Action::ClosePopup);
                }
            }
            Input::FinishClicked => self.finish_clicked(&mut out),
            Input::CancelClicked => out.push(Action::CloseWindow),
            Input::WindowClosed => self.window_closed(&mut out),
            Input::TimerFired(id) => self.timer_fired(id, &mut out),
            Input::CookiesRead(cookies) => self.cookies_read(&cookies, &mut out),
            Input::AdoptFinished(result) => self.adopt_finished(result, &mut out),
        }
        self.publish(&mut out);
        out
    }

    fn publish(&mut self, out: &mut Vec<Action>) {
        let vm = self.view_model();
        if self.last_view.as_ref() != Some(&vm) {
            self.last_view = Some(vm.clone());
            out.push(Action::UpdateView(vm));
        }
    }

    fn bail(&self) -> bool {
        self.dismissed || matches!(self.phase, Phase::Finishing | Phase::SignedIn { .. })
    }

    fn navigation_finished(&mut self, view: ViewKind, raw: &str, out: &mut Vec<Action>) {
        let Ok(url) = Url::parse(raw) else { return };
        if view == ViewKind::Main {
            self.main_url = Some(url.clone());
        }
        if self.gate.note(&url) != GateEvent::ReturnPage {
            return;
        }
        if view == ViewKind::Main {
            self.did_navigate_main_after_auth = true;
            if !self.nav.has_popup() {
                self.fire_return(url.as_str(), out);
            }
        }
    }

    fn popup_closed(&mut self, out: &mut Vec<Action>) {
        self.nav.popup_depth = self.nav.popup_depth.saturating_sub(1);
        if self.did_navigate_main_after_auth
            && !self.nav.has_popup()
            && let Some(main) = self.main_url.clone()
            && self.gate.matches_return_page(&main)
        {
            self.fire_return(main.as_str(), out);
        }
    }

    /// `onReturned` latch: at most once until re-armed.
    fn fire_return(&mut self, url: &str, out: &mut Vec<Action>) {
        if self.did_fire_return {
            return;
        }
        self.did_fire_return = true;
        self.auto_retry_used = false;
        out.push(Action::OnReturned(url.to_owned()));
        self.capture_after_return(out);
    }

    fn rearm_return(&mut self) {
        self.did_fire_return = false;
    }

    /// `captureAfterReturn`, steps 1 and 3. The popup wait (step 2) runs when
    /// the return delay expires, which is the only moment a popup can be open
    /// (`onReturned` itself only fires with no popup open).
    fn capture_after_return(&mut self, out: &mut Vec<Action>) {
        if self.bail() {
            return;
        }
        self.start_return_delay(out);
    }

    fn start_return_delay(&mut self, out: &mut Vec<Action>) {
        self.flow = Flow::ReturnDelay;
        out.push(Action::StartTimer {
            id: TimerId::ReturnDelay,
            delay: self.return_delay,
        });
    }

    fn timer_fired(&mut self, id: TimerId, out: &mut Vec<Action>) {
        match id {
            TimerId::StatusHint => self.hint_shown = true,
            TimerId::PopupPoll => {
                if let Flow::PopupWait { polls } = self.flow {
                    let polls = polls + 1;
                    if self.nav.has_popup() && polls < POPUP_POLL_MAX {
                        self.flow = Flow::PopupWait { polls };
                        out.push(Action::StartTimer {
                            id: TimerId::PopupPoll,
                            delay: POPUP_POLL_INTERVAL,
                        });
                    } else if !self.bail() {
                        // The popup closed, or the 8 s budget is spent.
                        self.begin_capture(true, out);
                    }
                }
            }
            TimerId::ReturnDelay => {
                if self.flow == Flow::ReturnDelay && !self.bail() {
                    if self.nav.has_popup() {
                        self.flow = Flow::PopupWait { polls: 0 };
                        out.push(Action::StartTimer {
                            id: TimerId::PopupPoll,
                            delay: POPUP_POLL_INTERVAL,
                        });
                    } else {
                        self.begin_capture(true, out);
                    }
                }
            }
            TimerId::CaptureAttempt => {
                if let Flow::Reading { attempt, automatic } = self.flow {
                    self.flow = Flow::Reading {
                        attempt: attempt + 1,
                        automatic,
                    };
                    out.push(Action::ReadCookies);
                }
            }
            TimerId::AutoRetry => {
                if self.flow == Flow::AutoRetryWait {
                    // Phase is still Finishing here; only dismissal bails.
                    self.start_reading(true, out);
                }
            }
            TimerId::ConfirmDismiss => {
                if matches!(self.phase, Phase::SignedIn { .. }) {
                    out.push(Action::CloseWindow);
                }
            }
        }
    }

    fn finish_clicked(&mut self, out: &mut Vec<Action>) {
        if self.bail() {
            return;
        }
        for id in [TimerId::PopupPoll, TimerId::ReturnDelay] {
            out.push(Action::CancelTimer(id));
        }
        self.begin_capture(false, out);
    }

    fn begin_capture(&mut self, automatic: bool, out: &mut Vec<Action>) {
        self.phase = Phase::Finishing;
        self.start_reading(automatic, out);
    }

    fn start_reading(&mut self, automatic: bool, out: &mut Vec<Action>) {
        self.flow = Flow::Reading {
            attempt: 1,
            automatic,
        };
        out.push(Action::ReadCookies);
    }

    fn cookies_read(&mut self, cookies: &[WebCookie], out: &mut Vec<Action>) {
        let Flow::Reading { attempt, automatic } = self.flow else {
            return;
        };
        if let Some(captured) = cookies::capture(cookies, &self.policy) {
            self.flow = Flow::Adopting { automatic };
            out.push(Action::Adopt(captured));
        } else if attempt < self.policy.max_attempts() {
            out.push(Action::StartTimer {
                id: TimerId::CaptureAttempt,
                delay: self.policy.retry_delay(),
            });
        } else {
            let msg = self.policy.failure_message().to_owned();
            self.capture_failed(automatic, msg, out);
        }
    }

    fn adopt_finished(&mut self, result: Result<Option<String>, String>, out: &mut Vec<Action>) {
        let Flow::Adopting { automatic } = self.flow else {
            return;
        };
        match result {
            Ok(email) => {
                self.flow = Flow::Idle;
                // confirmAndDismiss
                self.phase = Phase::SignedIn { email };
                out.push(Action::NotifyComplete);
                out.push(Action::StartTimer {
                    id: TimerId::ConfirmDismiss,
                    delay: CONFIRM_DISMISS_DELAY,
                });
            }
            Err(message) => {
                let message = if message.is_empty() {
                    self.policy.failure_message().to_owned()
                } else {
                    message
                };
                self.capture_failed(automatic, message, out);
            }
        }
    }

    /// Automatic captures retry once after 1.2 s; manual ones fail at once.
    fn capture_failed(&mut self, automatic: bool, message: String, out: &mut Vec<Action>) {
        if automatic && !self.auto_retry_used {
            self.auto_retry_used = true;
            self.flow = Flow::AutoRetryWait;
            out.push(Action::StartTimer {
                id: TimerId::AutoRetry,
                delay: AUTO_RETRY_DELAY,
            });
            return;
        }
        self.flow = Flow::Idle;
        self.phase = Phase::Failed { message };
        self.rearm_return();
    }

    fn window_closed(&mut self, out: &mut Vec<Action>) {
        self.dismissed = true;
        self.flow = Flow::Idle;
        for id in [
            TimerId::StatusHint,
            TimerId::PopupPoll,
            TimerId::ReturnDelay,
            TimerId::CaptureAttempt,
            TimerId::AutoRetry,
            TimerId::ConfirmDismiss,
        ] {
            out.push(Action::CancelTimer(id));
        }
    }
}
