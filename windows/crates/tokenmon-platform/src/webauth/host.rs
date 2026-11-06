//! The seam between the app and the sign-in window: traits, plus a headless
//! implementation that runs the real [`SignInMachine`] against a scripted
//! cookie jar and virtual timers. The Windows WebView2 adapter implements
//! [`WebAuthHost`]; tests and `tokenmon-app` use [`FakeWebAuthHost`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::config::SignInConfig;
use super::cookies::{CapturedCookies, WebCookie};
use super::machine::{Action, Input, Phase, SignInMachine, TimerId};

/// App-side callbacks of one sign-in window. Called on the window's thread;
/// keep them quick and non-blocking.
pub trait SignInDelegate: Send + Sync {
    /// The return page was detected (provider `onReturned`), before capture.
    fn on_returned(&self, _url: &str) {}

    /// Saves the captured session (`adopt`) and runs the provider's
    /// after-capture step. `Ok` carries the account e-mail; `Err` the message
    /// to show.
    fn adopt(&self, captured: CapturedCookies) -> Result<Option<String>, String>;

    /// Sign-in succeeded (`onComplete`).
    fn on_complete(&self) {}

    /// The window closed (any reason, success or not).
    fn on_closed(&self) {}
}

/// A sign-in window could not be opened or controlled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebAuthError {
    /// The start URL or provider slug is not acceptable.
    #[error("invalid sign-in configuration: {0}")]
    InvalidConfig(String),
    /// WebView2 is not installed or failed to start.
    #[error("the embedded browser is unavailable: {0}")]
    BrowserUnavailable(String),
    /// Not supported on this OS.
    #[error("embedded sign-in is only available on Windows")]
    Unsupported,
}

/// Opens provider sign-in windows and signs providers out.
pub trait WebAuthHost: Send + Sync {
    /// Opens the window for `config.provider_slug`, or focuses the existing
    /// one (one window per sign-in session).
    fn open(
        &self,
        config: SignInConfig,
        delegate: Arc<dyn SignInDelegate>,
    ) -> Result<(), WebAuthError>;

    /// Scoped sign-out: deletes this provider's browser state (open window
    /// cookies and leftover profile folders) and nothing else.
    fn sign_out(&self, provider_slug: &str);
}

/// Runs a [`SignInMachine`] without a browser: the test double of the
/// WebView2 adapter. Time is virtual and advanced explicitly.
pub struct HeadlessSignIn {
    machine: SignInMachine,
    delegate: Arc<dyn SignInDelegate>,
    jar: Vec<WebCookie>,
    now: Duration,
    timers: BTreeMap<u8, (Duration, TimerId)>,
    closed: bool,
    popups: usize,
    /// Every action the machine emitted, in order (excluding view updates).
    pub log: Vec<Action>,
}

fn timer_key(id: TimerId) -> u8 {
    match id {
        TimerId::StatusHint => 0,
        TimerId::PopupPoll => 1,
        TimerId::ReturnDelay => 2,
        TimerId::CaptureAttempt => 3,
        TimerId::AutoRetry => 4,
        TimerId::ConfirmDismiss => 5,
    }
}

impl HeadlessSignIn {
    /// A headless session; sends [`Input::Opened`] immediately.
    pub fn new(config: &SignInConfig, delegate: Arc<dyn SignInDelegate>) -> Self {
        let mut s = Self {
            machine: SignInMachine::new(config),
            delegate,
            jar: Vec::new(),
            now: Duration::ZERO,
            timers: BTreeMap::new(),
            closed: false,
            popups: 0,
            log: Vec::new(),
        };
        s.send(Input::Opened);
        s
    }

    /// The machine (inspect phase, view model).
    pub fn machine(&self) -> &SignInMachine {
        &self.machine
    }

    /// The window was closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Replaces the browser's cookie jar.
    pub fn set_cookies(&mut self, cookies: Vec<WebCookie>) {
        self.jar = cookies;
    }

    /// Feeds an input and executes the resulting actions.
    pub fn send(&mut self, input: Input) {
        let mut queue = vec![input];
        while let Some(i) = queue.pop() {
            for action in self.machine.handle(i) {
                self.execute(action, &mut queue);
            }
        }
    }

    /// Advances virtual time, firing due timers in order.
    pub fn advance(&mut self, by: Duration) {
        let target = self.now + by;
        loop {
            let next = self
                .timers
                .iter()
                .filter(|(_, (due, _))| *due <= target)
                .min_by_key(|(_, (due, _))| *due)
                .map(|(k, (due, id))| (*k, *due, *id));
            let Some((key, due, id)) = next else { break };
            self.timers.remove(&key);
            self.now = due;
            self.send(Input::TimerFired(id));
        }
        self.now = target;
    }

    fn execute(&mut self, action: Action, queue: &mut Vec<Input>) {
        match &action {
            Action::StartTimer { id, delay } => {
                self.timers.insert(timer_key(*id), (self.now + *delay, *id));
            }
            Action::CancelTimer(id) => {
                self.timers.remove(&timer_key(*id));
            }
            Action::ReadCookies => queue.push(Input::CookiesRead(self.jar.clone())),
            Action::Adopt(captured) => {
                queue.push(Input::AdoptFinished(self.delegate.adopt(captured.clone())));
            }
            Action::OnReturned(url) => self.delegate.on_returned(url),
            Action::NotifyComplete => self.delegate.on_complete(),
            Action::CloseWindow => {
                if !self.closed {
                    self.closed = true;
                    self.delegate.on_closed();
                    queue.push(Input::WindowClosed);
                }
            }
            Action::ClosePopup => {
                if self.popups > 0 {
                    queue.push(Input::PopupClosed);
                }
            }
            Action::GoBack | Action::UpdateView(_) => {}
        }
        if !matches!(action, Action::UpdateView(_)) {
            self.log.push(action);
        }
    }

    /// Simulates the browser opening a popup.
    pub fn open_popup(&mut self) {
        self.popups += 1;
        self.send(Input::PopupOpened);
    }

    /// Simulates a popup closing itself (`window.close()`).
    pub fn close_popup(&mut self) {
        self.popups = self.popups.saturating_sub(1);
        self.send(Input::PopupClosed);
    }

    /// The phase, cloned.
    pub fn phase(&self) -> Phase {
        self.machine.phase().clone()
    }
}

/// Recording [`WebAuthHost`] for app tests: remembers opens and sign-outs
/// and keeps the headless sessions so tests can drive them.
#[derive(Default)]
pub struct FakeWebAuthHost {
    /// Providers opened, in order.
    pub opened: Mutex<Vec<String>>,
    /// Providers signed out, in order.
    pub signed_out: Mutex<Vec<String>>,
    /// The live headless sessions by provider slug.
    pub sessions: Mutex<BTreeMap<String, Arc<Mutex<HeadlessSignIn>>>>,
}

impl WebAuthHost for FakeWebAuthHost {
    fn open(
        &self,
        config: SignInConfig,
        delegate: Arc<dyn SignInDelegate>,
    ) -> Result<(), WebAuthError> {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        let live = sessions
            .get(&config.provider_slug)
            .is_some_and(|s| !s.lock().expect("session lock").is_closed());
        if !live {
            sessions.insert(
                config.provider_slug.clone(),
                Arc::new(Mutex::new(HeadlessSignIn::new(&config, delegate))),
            );
        }
        self.opened
            .lock()
            .expect("opened lock")
            .push(config.provider_slug);
        Ok(())
    }

    fn sign_out(&self, provider_slug: &str) {
        self.signed_out
            .lock()
            .expect("signed_out lock")
            .push(provider_slug.to_owned());
        self.sessions
            .lock()
            .expect("sessions lock")
            .remove(provider_slug);
    }
}
