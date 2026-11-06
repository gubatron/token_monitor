//! Sleep/wake gate, event decoding and the connectivity mapping (ws-power).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokenmon_core::polling::testing::ManualScheduler;
use tokenmon_core::polling::{PollOutcome, PollingLoop};
use tokenmon_platform::power::event::{RESUME_DEDUPE_WINDOW, network_flags_satisfied};
use tokenmon_platform::power::{
    FakeEventSource, GateAction, GateCallbacks, GateMachine, NetworkLevel, PowerEvent,
    ResumeDeduper, SystemEventSource, SystemWakeGate, decode_power_broadcast,
};
use tokio::runtime::Handle;

type Events = Arc<Mutex<Vec<&'static str>>>;

fn events(log: &Events) -> Vec<&'static str> {
    log.lock().expect("event log").clone()
}

/// A gate wired to a manual scheduler, recording "sleep"/"ready".
fn harness() -> (Arc<SystemWakeGate>, ManualScheduler, Events) {
    let log: Events = Arc::default();
    let (sleep_log, ready_log) = (Arc::clone(&log), Arc::clone(&log));
    let scheduler = ManualScheduler::new();
    let gate = SystemWakeGate::new(
        Arc::new(scheduler.clone()),
        Handle::current(),
        GateCallbacks {
            on_sleep: Arc::new(move || sleep_log.lock().expect("log").push("sleep")),
            on_ready: Arc::new(move || ready_log.lock().expect("log").push("ready")),
        },
    );
    (Arc::new(gate), scheduler, log)
}

fn started() -> (
    FakeEventSource,
    Arc<SystemWakeGate>,
    ManualScheduler,
    Events,
) {
    let (gate, scheduler, log) = harness();
    let source = FakeEventSource::new();
    source.start(gate.clone());
    (source, gate, scheduler, log)
}

/// Swift `testWakeWaitsForASatisfiedPath`.
#[tokio::test(flavor = "current_thread")]
async fn test_wake_waits_for_a_satisfied_path() {
    let (source, _gate, scheduler, log) = started();
    source.sleep();
    source.wake();
    assert_eq!(events(&log), ["sleep"]);
    source.path(false);
    assert_eq!(events(&log), ["sleep"]);
    source.path(true);
    assert_eq!(events(&log), ["sleep", "ready"]);
    scheduler.settle().await;
    assert_eq!(
        scheduler.pending_count(),
        0,
        "the fallback wait is cancelled"
    );
}

/// Swift `testPathThatCameBackDuringWakeResumesImmediately`.
#[tokio::test(flavor = "current_thread")]
async fn test_path_that_came_back_during_wake_resumes_immediately() {
    let (source, _gate, _scheduler, log) = started();
    source.sleep();
    source.path(true);
    source.wake();
    assert_eq!(events(&log), ["sleep", "ready"]);
}

/// Swift `testFallbackResumesWhenThePathNeverChanges`.
#[tokio::test(flavor = "current_thread")]
async fn test_fallback_resumes_when_the_path_never_changes() {
    let (source, _gate, scheduler, log) = started();
    source.sleep();
    source.wake();
    assert_eq!(scheduler.next_sleep().await, Duration::from_secs(30));
    scheduler.fire();
    scheduler.settle().await;
    assert_eq!(events(&log), ["sleep", "ready"]);
}

/// Swift `testPathChangesOutsideAWakeDoNothing`.
#[tokio::test(flavor = "current_thread")]
async fn test_path_changes_outside_a_wake_do_nothing() {
    let (source, _gate, _scheduler, log) = started();
    source.path(false);
    source.path(true);
    assert_eq!(events(&log).len(), 0);
}

/// A second sleep before the fallback fires cancels it: no stray "ready".
#[tokio::test(flavor = "current_thread")]
async fn test_sleep_during_wake_cancels_the_fallback() {
    let (source, gate, scheduler, log) = started();
    source.sleep();
    source.wake();
    source.sleep();
    scheduler.settle().await;
    assert_eq!(scheduler.pending_count(), 0);
    assert!(!gate.is_awaiting_network());
    assert_eq!(events(&log), ["sleep", "sleep"]);
}

/// A repeated wake restarts the fallback; only one "ready" results.
#[tokio::test(flavor = "current_thread")]
async fn test_repeated_wake_restarts_the_fallback_once() {
    let (source, _gate, scheduler, log) = started();
    source.sleep();
    source.wake();
    source.wake();
    scheduler.settle().await;
    assert_eq!(scheduler.pending_count(), 1);
    assert_eq!(scheduler.cancelled_count(), 1);
    scheduler.fire();
    scheduler.settle().await;
    assert_eq!(events(&log), ["sleep", "ready"]);
}

/// Dropping the gate cancels the pending fallback.
#[tokio::test(flavor = "current_thread")]
async fn test_dropping_the_gate_cancels_the_fallback() {
    let (source, gate, scheduler, log) = started();
    source.sleep();
    source.wake();
    scheduler.settle().await;
    drop(source);
    drop(gate);
    scheduler.settle().await;
    assert_eq!(scheduler.pending_count(), 0);
    assert_eq!(events(&log), ["sleep"]);
}

/// The gate pauses and resumes a real `PollingLoop`, which refreshes at once
/// when overdue (spec 6.5).
#[tokio::test(flavor = "current_thread")]
async fn test_gate_pauses_and_resumes_a_polling_loop() {
    let time = ManualScheduler::new();
    let count = Arc::new(Mutex::new(0u32));
    let c = Arc::clone(&count);
    let poller = PollingLoop::builder(
        || Some(Duration::from_secs(60)),
        move || {
            let c = Arc::clone(&c);
            async move {
                *c.lock().expect("count") += 1;
                PollOutcome::Success
            }
        },
    )
    .clock(time.clock())
    .sleeper(Arc::new(time.clone()))
    .jitter(Arc::new(|| 0.0))
    .build();
    poller.start();
    assert_eq!(time.next_sleep().await, Duration::from_secs(60));
    assert_eq!(*count.lock().expect("count"), 1);

    let (p_sleep, p_ready) = (Arc::clone(&poller), Arc::clone(&poller));
    let gate = Arc::new(SystemWakeGate::new(
        Arc::new(ManualScheduler::new()),
        Handle::current(),
        GateCallbacks {
            on_sleep: Arc::new(move || p_sleep.pause()),
            on_ready: Arc::new(move || p_ready.resume()),
        },
    ));
    let source = FakeEventSource::new();
    source.start(gate.clone());
    source.sleep();
    time.settle().await;
    time.advance(Duration::from_secs(120));
    source.wake();
    source.path(true);
    time.settle().await;
    assert_eq!(
        *count.lock().expect("count"),
        2,
        "overdue loop refreshes on resume"
    );
    poller.stop();
}

/// Machine: the documented table, action by action.
#[test]
fn test_machine_actions_follow_the_spec_table() {
    let mut g = GateMachine::new();
    assert_eq!(
        g.will_sleep(),
        [GateAction::CancelFallback, GateAction::Sleep]
    );
    assert_eq!(g.did_wake(), [GateAction::StartFallback]);
    assert_eq!(g.path_changed(false).len(), 0);
    assert_eq!(
        g.fallback_fired(),
        [GateAction::CancelFallback, GateAction::Ready]
    );
    assert!(
        g.fallback_fired().is_empty(),
        "finishWake only while awaiting"
    );
}

/// Resume codes decode to a wake, suspend to a sleep, the rest to nothing.
#[test]
fn test_power_broadcast_decoding() {
    assert_eq!(decode_power_broadcast(0x04), Some(PowerEvent::WillSleep));
    assert_eq!(decode_power_broadcast(0x07), Some(PowerEvent::DidWake));
    assert_eq!(decode_power_broadcast(0x12), Some(PowerEvent::DidWake));
    for other in [0, 0x0A, 0x0B, 0x8013, u32::MAX] {
        assert_eq!(decode_power_broadcast(other), None);
    }
}

/// FEAT-power-resume-dedupe: the resume pair yields one wake; suspend yields a sleep.
#[test]
fn test_resume_pair_gives_one_wake_per_sleep() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut d = ResumeDeduper::default();
    assert_eq!(d.accept_at(0x04, t0), Some(PowerEvent::WillSleep));
    assert_eq!(d.accept_at(0x12, t0 + ms(10)), Some(PowerEvent::DidWake));
    assert_eq!(d.accept_at(0x07, t0 + ms(30)), None);
    assert_eq!(d.accept_at(0x12, t0 + ms(40)), None);
    assert_eq!(d.accept_at(0x04, t0 + ms(50)), Some(PowerEvent::WillSleep));
    // A suspend clears the window: a prompt resume still wakes.
    assert_eq!(d.accept_at(0x07, t0 + ms(60)), Some(PowerEvent::DidWake));
    assert_eq!(d.accept_at(0x12, t0 + ms(70)), None);
}

/// Regression (#67): consecutive Modern Standby resumes with no suspend
/// message between them each wake; only the pair within the window collapses.
#[test]
fn test_consecutive_modern_standby_resumes_each_wake() {
    let t0 = Instant::now();
    let mut d = ResumeDeduper::default();
    for minute in 0..3 {
        let at = t0 + Duration::from_secs(600 * minute);
        assert_eq!(d.accept_at(0x12, at), Some(PowerEvent::DidWake), "{minute}");
        assert_eq!(d.accept_at(0x07, at + Duration::from_millis(15)), None);
    }
}

/// The window boundary: a repeat just inside is dropped, just outside wakes.
#[test]
fn test_resume_dedupe_window_boundary() {
    let t0 = Instant::now();
    let mut d = ResumeDeduper::default();
    assert_eq!(d.accept_at(0x12, t0), Some(PowerEvent::DidWake));
    let inside = t0 + Duration::from_millis(4999);
    assert_eq!(d.accept_at(0x12, inside), None);
    assert_eq!(
        d.accept_at(0x12, t0 + RESUME_DEDUPE_WINDOW),
        Some(PowerEvent::DidWake)
    );
}

/// Unknown codes never change the state.
#[test]
fn test_resume_deduper_ignores_unknown_codes() {
    let mut d = ResumeDeduper::default();
    assert_eq!(d.accept(0x0A), None);
    assert_eq!(d.accept(0x12), Some(PowerEvent::DidWake));
}

/// Modern Standby: a resume with no suspend message still wakes once.
#[test]
fn test_modern_standby_resume_without_suspend_wakes_once() {
    let mut d = ResumeDeduper::default();
    assert_eq!(d.accept(0x12), Some(PowerEvent::DidWake));
    assert_eq!(d.accept(0x12), None);
}

/// Repeated suspend/resume cycles: every cycle delivers exactly one sleep and
/// one wake, whichever resume message (or both) Windows sends.
#[test]
fn test_repeated_suspend_resume_cycles_each_deliver_one_wake() {
    let mut d = ResumeDeduper::default();
    for resume_messages in [&[0x12, 0x07][..], &[0x07, 0x12], &[0x12], &[0x07]] {
        assert_eq!(d.accept(0x04), Some(PowerEvent::WillSleep));
        let wakes = resume_messages
            .iter()
            .filter_map(|m| d.accept(*m))
            .filter(|e| *e == PowerEvent::DidWake)
            .count();
        assert_eq!(wakes, 1, "one wake for resume messages {resume_messages:?}");
    }
}

/// Modern Standby sends resumes with no suspend message in between: each wake
/// must still be delivered, not just the first one of the process.
#[test]
#[ignore = "blocked on #67: ResumeDeduper drops every wake after the first until PBT_APMSUSPEND"]
fn test_repeated_modern_standby_wakes_are_each_delivered() {
    let mut d = ResumeDeduper::default();
    for cycle in 0..4 {
        assert_eq!(
            d.accept(0x12),
            Some(PowerEvent::DidWake),
            "modern standby wake {cycle} was dropped"
        );
    }
}

/// FEAT-power-network-satisfied: only IPv4/IPv6 internet counts.
#[test]
fn test_network_satisfied_means_internet_connectivity() {
    assert!(network_flags_satisfied(0x40));
    assert!(network_flags_satisfied(0x400));
    assert!(network_flags_satisfied(0x440 | 0x1));
    assert!(!network_flags_satisfied(0));
    assert!(
        !network_flags_satisfied(0x10 | 0x100),
        "local-only is not satisfied"
    );
    assert!(NetworkLevel::from_raw(3).is_satisfied());
    for raw in [-1, 0, 1, 2, 4, 5, 99] {
        assert!(!NetworkLevel::from_raw(raw).is_satisfied(), "{raw}");
    }
}
