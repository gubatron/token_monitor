//! State machine and timings (spec 04 4.2-4.4), run through the headless
//! driver with virtual time.

#![allow(
    clippy::unwrap_used,
    clippy::unchecked_time_subtraction,
    clippy::cloned_ref_to_slice_refs,
    clippy::const_is_empty
)]

mod webauth_common;

use std::sync::{Arc, Mutex};

use tokenmon_platform::webauth::cookies::CapturedCookies;
use tokenmon_platform::webauth::host::{
    FakeWebAuthHost, HeadlessSignIn, SignInDelegate, WebAuthHost,
};
use tokenmon_platform::webauth::machine::{
    AUTO_RETRY_DELAY, Action, CONFIRM_DISMISS_DELAY, Input, POPUP_POLL_INTERVAL, POPUP_POLL_MAX,
    Phase, STATUS_HINT_DELAY, SignInMachine, TimerId, ViewKind,
};
use tokenmon_platform::webauth::nav::NavSnapshot;
use webauth_common::*;

#[derive(Default)]
struct Delegate {
    adopted: Mutex<Vec<CapturedCookies>>,
    returned: Mutex<Vec<String>>,
    completes: Mutex<u32>,
    closes: Mutex<u32>,
    /// Errors to return from `adopt` before succeeding.
    fail_next: Mutex<Vec<String>>,
}

impl SignInDelegate for Delegate {
    fn on_returned(&self, url: &str) {
        self.returned.lock().unwrap().push(url.to_owned());
    }
    fn adopt(&self, c: CapturedCookies) -> Result<Option<String>, String> {
        if let Some(e) = self.fail_next.lock().unwrap().pop() {
            return Err(e);
        }
        // The app resolves the account e-mail after capture.
        self.adopted.lock().unwrap().push(c);
        Ok(Some("me@x.io".into()))
    }
    fn on_complete(&self) {
        *self.completes.lock().unwrap() += 1;
    }
    fn on_closed(&self) {
        *self.closes.lock().unwrap() += 1;
    }
}

fn session() -> (HeadlessSignIn, Arc<Delegate>) {
    let d = Arc::new(Delegate::default());
    (HeadlessSignIn::new(&claude_config(), d.clone()), d)
}

fn nav(s: &mut HeadlessSignIn, view: ViewKind, url: &str) {
    s.send(Input::NavigationFinished {
        view,
        url: url.into(),
    });
}

fn good_jar() -> Vec<tokenmon_platform::webauth::WebCookie> {
    vec![
        cookie("sessionKey", "sk", ".claude.ai"),
        cookie("lastActiveOrg", "org", ".claude.ai"),
    ]
}

fn login_then_return(s: &mut HeadlessSignIn) {
    nav(s, ViewKind::Main, "https://claude.ai/new");
    nav(s, ViewKind::Main, "https://claude.ai/login");
    nav(s, ViewKind::Main, "https://claude.ai/new");
}

#[test]
fn start_url_load_never_triggers_capture() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    nav(&mut s, ViewKind::Main, "https://claude.ai/new");
    s.advance(ms(60_000));
    assert!(d.returned.lock().unwrap().is_empty());
    assert_eq!(s.phase(), Phase::SigningIn);
}

#[test]
fn happy_path_waits_the_return_delay_then_captures_then_closes_after_1100ms() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    login_then_return(&mut s);
    assert_eq!(
        d.returned.lock().unwrap().as_slice(),
        ["https://claude.ai/new"]
    );
    assert_eq!(
        s.phase(),
        Phase::SigningIn,
        "capture waits for the return delay"
    );
    s.advance(ms(799));
    assert_eq!(s.phase(), Phase::SigningIn);
    s.advance(ms(1));
    // Capture and adoption are synchronous in the headless driver.
    assert_eq!(
        s.phase(),
        Phase::SignedIn {
            email: Some("me@x.io".into())
        }
    );
    assert_eq!(*d.completes.lock().unwrap(), 1);
    let adopted = d.adopted.lock().unwrap();
    assert_eq!(adopted[0].cookie_header, "sessionKey=sk; lastActiveOrg=org");
    drop(adopted);
    assert!(!s.is_closed());
    s.advance(CONFIRM_DISMISS_DELAY - ms(1));
    assert!(!s.is_closed());
    s.advance(ms(1));
    assert!(s.is_closed());
    assert_eq!(*d.closes.lock().unwrap(), 1);
    let vm = s.machine().view_model();
    assert_eq!(vm.status_text, "Signed in as me@x.io");
}

#[test]
fn grok_return_delay_is_1500ms() {
    let d = Arc::new(Delegate::default());
    let mut s = HeadlessSignIn::new(&grok_config(), d.clone());
    s.set_cookies(vec![cookie("sso", "t", ".grok.com")]);
    nav(&mut s, ViewKind::Main, "https://accounts.x.ai/sign-in");
    nav(&mut s, ViewKind::Main, "https://grok.com/?_s=usage");
    s.advance(ms(1499));
    assert_eq!(s.phase(), Phase::SigningIn);
    s.advance(ms(1));
    assert!(matches!(s.phase(), Phase::SignedIn { email: Some(_) }));
}

#[test]
fn return_fires_at_most_once_until_rearmed() {
    let (mut s, d) = session();
    s.set_cookies(vec![]);
    login_then_return(&mut s);
    nav(&mut s, ViewKind::Main, "https://claude.ai/chat/1");
    nav(&mut s, ViewKind::Main, "https://claude.ai/chat/2");
    assert_eq!(d.returned.lock().unwrap().len(), 1);
}

#[test]
fn popup_open_defers_capture_until_it_closes_then_applies_the_return_delay() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    nav(&mut s, ViewKind::Main, "https://claude.ai/login");
    s.open_popup();
    nav(&mut s, ViewKind::Popup, "https://accounts.google.com/x");
    // Main returns while the popup is open: no onReturned yet.
    nav(&mut s, ViewKind::Main, "https://claude.ai/new");
    assert!(d.returned.lock().unwrap().is_empty());
    assert_eq!(
        s.machine().status_text(),
        "Complete sign-in in the popup above."
    );
    s.close_popup();
    // Closing fires the return; the popup poll sees no popup after 200 ms.
    assert_eq!(d.returned.lock().unwrap().len(), 1);
    s.advance(ms(799));
    assert_eq!(s.phase(), Phase::SigningIn);
    s.advance(ms(1));
    assert!(matches!(s.phase(), Phase::SignedIn { .. }));
}

#[test]
fn popup_close_without_main_return_does_nothing() {
    let (mut s, d) = session();
    nav(&mut s, ViewKind::Main, "https://claude.ai/login");
    s.open_popup();
    s.close_popup();
    assert!(d.returned.lock().unwrap().is_empty());
}

#[test]
fn popup_close_requires_main_to_still_be_on_a_return_page() {
    let (mut s, d) = session();
    nav(&mut s, ViewKind::Main, "https://claude.ai/login");
    s.open_popup();
    nav(&mut s, ViewKind::Main, "https://claude.ai/new");
    nav(&mut s, ViewKind::Main, "https://claude.ai/login");
    s.close_popup();
    assert!(d.returned.lock().unwrap().is_empty());
}

#[test]
fn popup_return_page_alone_never_fires() {
    let (mut s, d) = session();
    nav(&mut s, ViewKind::Main, "https://claude.ai/login");
    s.open_popup();
    nav(&mut s, ViewKind::Popup, "https://claude.ai/new");
    s.close_popup();
    assert!(d.returned.lock().unwrap().is_empty());
}

#[test]
fn popup_opened_during_the_return_delay_is_waited_out_in_200ms_polls() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    login_then_return(&mut s);
    s.open_popup();
    s.advance(ms(800)); // return delay expires with a popup open
    assert_eq!(s.phase(), Phase::SigningIn);
    s.advance(ms(200 * 3));
    assert!(d.adopted.lock().unwrap().is_empty(), "still polling");
    s.close_popup();
    s.advance(ms(200));
    assert!(matches!(s.phase(), Phase::SignedIn { .. }));
}

#[test]
fn popup_poll_gives_up_after_forty_polls() {
    let (mut s, _d) = session();
    s.set_cookies(good_jar());
    login_then_return(&mut s);
    s.open_popup();
    s.advance(ms(800));
    s.advance(POPUP_POLL_INTERVAL * (POPUP_POLL_MAX - 1));
    assert_eq!(s.phase(), Phase::SigningIn, "39 polls: still waiting");
    s.advance(POPUP_POLL_INTERVAL);
    assert!(
        matches!(s.phase(), Phase::SignedIn { .. }),
        "40th poll: 8 s budget spent"
    );
    assert_eq!(
        POPUP_POLL_INTERVAL * POPUP_POLL_MAX,
        std::time::Duration::from_secs(8)
    );
}

#[test]
fn missing_cookies_retry_four_times_400ms_apart_then_auto_retry_after_1200ms() {
    let (mut s, d) = session();
    s.set_cookies(vec![]);
    login_then_return(&mut s);
    s.advance(ms(800)); // return delay: attempt 1 fails
    assert_eq!(s.phase(), Phase::Finishing);
    let reads = |s: &HeadlessSignIn| {
        s.log
            .iter()
            .filter(|a| matches!(a, Action::ReadCookies))
            .count()
    };
    assert_eq!(reads(&s), 1);
    s.advance(ms(400));
    assert_eq!(reads(&s), 2);
    s.advance(ms(800));
    assert_eq!(reads(&s), 4);
    assert_eq!(
        s.phase(),
        Phase::Finishing,
        "still waiting for the automatic retry"
    );
    s.advance(AUTO_RETRY_DELAY - ms(1));
    assert_eq!(reads(&s), 4);
    // Cookies arrive just before the retry.
    s.set_cookies(good_jar());
    s.advance(ms(1));
    assert!(
        matches!(s.phase(), Phase::SignedIn { .. }),
        "auto retry captured"
    );
    assert_eq!(d.adopted.lock().unwrap().len(), 1);
}

#[test]
fn auto_capture_failure_after_retry_shows_message_and_rearms_the_return() {
    let (mut s, d) = session();
    s.set_cookies(vec![]);
    login_then_return(&mut s);
    s.advance(ms(800 + 1200 + 1200 + 1200 + 5000));
    assert_eq!(
        s.phase(),
        Phase::Failed {
            message: "No session yet.".into()
        }
    );
    let vm = s.machine().view_model();
    assert_eq!(vm.primary_title, "Try Again");
    assert!(vm.primary_enabled);
    assert_eq!(vm.status_text, "No session yet.");
    // Rearmed: the next return fires again and succeeds.
    s.set_cookies(good_jar());
    nav(&mut s, ViewKind::Main, "https://claude.ai/chat/2");
    assert_eq!(d.returned.lock().unwrap().len(), 2);
    s.advance(ms(800));
    assert!(matches!(s.phase(), Phase::SignedIn { .. }));
}

#[test]
fn delegate_error_is_shown_and_manual_retry_works() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    d.fail_next.lock().unwrap().push("vault locked".into());
    s.send(Input::FinishClicked);
    assert_eq!(
        s.phase(),
        Phase::Failed {
            message: "vault locked".into()
        },
        "manual: no auto retry"
    );
    s.send(Input::FinishClicked);
    assert!(matches!(s.phase(), Phase::SignedIn { .. }));
}

#[test]
fn empty_delegate_error_falls_back_to_the_default_message() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    d.fail_next.lock().unwrap().push(String::new());
    s.send(Input::FinishClicked);
    assert_eq!(
        s.phase(),
        Phase::Failed {
            message: "No session yet.".into()
        }
    );
}

#[test]
fn manual_finish_with_no_session_retries_attempts_then_fails_without_auto_retry() {
    let (mut s, _d) = session();
    s.set_cookies(vec![]);
    s.send(Input::FinishClicked);
    assert_eq!(s.phase(), Phase::Finishing);
    assert_eq!(s.machine().view_model().primary_title, "Finishing\u{2026}");
    assert!(!s.machine().view_model().primary_enabled);
    assert!(s.machine().view_model().finishing_spinner);
    s.advance(ms(1200));
    assert!(matches!(s.phase(), Phase::Failed { .. }));
}

#[test]
fn finish_is_ignored_while_finishing_or_signed_in_or_dismissed() {
    let (mut s, d) = session();
    s.set_cookies(vec![]);
    s.send(Input::FinishClicked);
    let n = s.log.len();
    s.send(Input::FinishClicked);
    assert_eq!(s.log.len(), n);
    s.set_cookies(good_jar());
    s.advance(ms(400));
    assert!(matches!(s.phase(), Phase::SignedIn { .. }));
    let n = s.log.len();
    s.send(Input::FinishClicked);
    assert_eq!(s.log.len(), n);
    assert_eq!(d.adopted.lock().unwrap().len(), 1);

    let (mut s2, d2) = session();
    s2.set_cookies(good_jar());
    s2.send(Input::WindowClosed);
    s2.send(Input::FinishClicked);
    assert!(d2.adopted.lock().unwrap().is_empty());
}

#[test]
fn manual_finish_cancels_the_pending_automatic_capture() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    login_then_return(&mut s);
    s.send(Input::FinishClicked);
    assert!(matches!(s.phase(), Phase::SignedIn { .. }));
    s.advance(ms(5000));
    assert_eq!(d.adopted.lock().unwrap().len(), 1, "no second capture");
}

#[test]
fn dismissing_the_window_cancels_everything() {
    let (mut s, d) = session();
    s.set_cookies(good_jar());
    login_then_return(&mut s);
    s.send(Input::WindowClosed);
    s.advance(ms(60_000));
    assert!(d.adopted.lock().unwrap().is_empty());
    assert!(s.machine().is_dismissed());
}

#[test]
fn status_text_follows_the_phase_popup_and_25s_hint() {
    let (mut s, _d) = session();
    assert_eq!(s.machine().status_text(), "Sign in to your Claude account.");
    s.advance(STATUS_HINT_DELAY - ms(1));
    assert_eq!(s.machine().status_text(), "Sign in to your Claude account.");
    s.advance(ms(1));
    assert_eq!(
        s.machine().status_text(),
        "Signed in? Choose Finish Sign-In to complete."
    );
    s.open_popup();
    assert_eq!(
        s.machine().status_text(),
        "Complete sign-in in the popup above."
    );
    s.close_popup();
    assert_eq!(
        s.machine().status_text(),
        "Signed in? Choose Finish Sign-In to complete."
    );
    s.set_cookies(vec![]);
    s.send(Input::FinishClicked);
    assert_eq!(s.machine().status_text(), "Finishing sign-in\u{2026}");
}

#[test]
fn signed_in_view_model_shows_check_and_disabled_button() {
    let (mut s, _d) = session();
    s.set_cookies(good_jar());
    s.send(Input::FinishClicked);
    let vm = s.machine().view_model();
    assert!(vm.show_check);
    assert_eq!(vm.primary_title, "Signed In");
    assert!(!vm.primary_enabled);
    assert_eq!(vm.title, "Sign in to Claude");
}

// testApplyDoesNotPublishWhenNothingChanged / PublishesOnceForEachRealChange
#[test]
fn view_updates_are_published_only_when_something_changed() {
    let mut m = SignInMachine::new(&claude_config());
    let snap = NavSnapshot {
        can_go_back: false,
        can_go_forward: false,
        is_loading: false,
        current_url: Some("https://claude.ai/new".into()),
    };
    let updates = |a: &[Action]| {
        a.iter()
            .filter(|x| matches!(x, Action::UpdateView(_)))
            .count()
    };
    assert_eq!(updates(&m.handle(Input::NavSnapshot(snap.clone()))), 1);
    for _ in 0..5 {
        assert_eq!(updates(&m.handle(Input::NavSnapshot(snap.clone()))), 0);
    }
    let a = m.handle(Input::PopupOpened);
    assert_eq!(updates(&a), 1);
    let vm = m.view_model();
    assert!(vm.back_enabled && vm.show_close_popup);
    assert_eq!(vm.address, "Sign-in popup");
    let mut moved = snap;
    moved.current_url = Some("https://claude.ai/chat".into());
    m.handle(Input::PopupClosed);
    assert_eq!(updates(&m.handle(Input::NavSnapshot(moved))), 1);
    assert_eq!(m.view_model().address, "claude.ai/chat");
}

// testAttachIsIdempotent / testRearmReturnDoesNotPublish
#[test]
fn repeated_opened_and_failed_rearm_publish_nothing_extra() {
    let mut m = SignInMachine::new(&claude_config());
    let a = m.handle(Input::Opened);
    assert!(a.iter().any(|x| matches!(x, Action::UpdateView(_))));
    for _ in 0..5 {
        let a = m.handle(Input::Opened);
        assert!(!a.iter().any(|x| matches!(x, Action::UpdateView(_))));
    }
}

#[test]
fn back_and_close_popup_buttons_emit_the_right_actions() {
    let mut m = SignInMachine::new(&claude_config());
    assert!(
        m.handle(Input::BackClicked)
            .iter()
            .all(|a| matches!(a, Action::UpdateView(_)))
    );
    assert!(
        m.handle(Input::ClosePopupClicked)
            .iter()
            .all(|a| matches!(a, Action::UpdateView(_)))
    );
    m.handle(Input::PopupOpened);
    assert!(m.handle(Input::BackClicked).contains(&Action::ClosePopup));
    assert!(
        m.handle(Input::ClosePopupClicked)
            .contains(&Action::ClosePopup)
    );
    m.handle(Input::NavSnapshot(NavSnapshot {
        can_go_back: true,
        ..NavSnapshot::default()
    }));
    assert!(m.handle(Input::BackClicked).contains(&Action::GoBack));
    assert!(
        m.handle(Input::CancelClicked)
            .contains(&Action::CloseWindow)
    );
}

#[test]
fn nested_popups_pop_one_at_a_time() {
    let mut m = SignInMachine::new(&claude_config());
    m.handle(Input::PopupOpened);
    m.handle(Input::PopupOpened);
    assert_eq!(m.popup_depth(), 2);
    m.handle(Input::PopupClosed);
    assert_eq!(m.popup_depth(), 1);
    m.handle(Input::PopupClosed);
    m.handle(Input::PopupClosed); // stray close never underflows
    assert_eq!(m.popup_depth(), 0);
}

#[test]
fn invalid_urls_and_stray_results_are_ignored() {
    let mut m = SignInMachine::new(&claude_config());
    m.handle(Input::NavigationFinished {
        view: ViewKind::Main,
        url: "::::".into(),
    });
    m.handle(Input::CookiesRead(vec![cookie(
        "sessionKey",
        "v",
        "claude.ai",
    )]));
    m.handle(Input::AdoptFinished(Ok(None)));
    m.handle(Input::TimerFired(TimerId::CaptureAttempt));
    m.handle(Input::TimerFired(TimerId::ConfirmDismiss));
    assert_eq!(m.phase(), &Phase::SigningIn);
}

#[test]
fn fake_host_records_opens_and_scoped_sign_outs() {
    let host = FakeWebAuthHost::default();
    let d = Arc::new(Delegate::default());
    host.open(claude_config(), d.clone()).unwrap();
    host.open(claude_config(), d).unwrap(); // focuses the existing one
    assert_eq!(host.sessions.lock().unwrap().len(), 1);
    assert_eq!(host.opened.lock().unwrap().len(), 2);
    host.sign_out("claude");
    assert_eq!(host.signed_out.lock().unwrap().as_slice(), ["claude"]);
    assert!(host.sessions.lock().unwrap().is_empty());
}
