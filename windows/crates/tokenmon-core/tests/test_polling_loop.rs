//! `BackoffTimer` and `PollingLoop` behaviour, driven by a manual clock and
//! sleeper so no test waits on real time (ports of BackoffTimerTests and
//! PollingLoopTests).
#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokenmon_core::polling::testing::ManualScheduler;
use tokenmon_core::polling::{
    BackoffTimer, JitterSource, PollOutcome, PollingLoop, RefreshError, Sleeper, TokioSleeper,
};
use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

/// Shared knobs the loop's closures read, plus a recorded refresh count.
struct Harness {
    time: ManualScheduler,
    interval: Arc<Mutex<Option<Duration>>>,
    outcomes: Arc<Mutex<Vec<PollOutcome>>>,
    jitter: Arc<Mutex<f64>>,
    refreshes: Arc<AtomicUsize>,
}

impl Harness {
    fn new() -> Self {
        Self {
            time: ManualScheduler::new(),
            interval: Arc::new(Mutex::new(Some(secs(300)))),
            outcomes: Arc::default(),
            jitter: Arc::new(Mutex::new(0.0)),
            refreshes: Arc::default(),
        }
    }

    fn set_interval(&self, v: Option<u64>) {
        *self.interval.lock().unwrap() = v.map(secs);
    }

    fn count(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }

    fn make(&self) -> Arc<PollingLoop> {
        let interval = Arc::clone(&self.interval);
        let outcomes = Arc::clone(&self.outcomes);
        let refreshes = Arc::clone(&self.refreshes);
        let jitter = Arc::clone(&self.jitter);
        PollingLoop::builder(
            move || *interval.lock().unwrap(),
            move || {
                refreshes.fetch_add(1, Ordering::SeqCst);
                let mut queue = outcomes.lock().unwrap();
                let next = if queue.is_empty() {
                    PollOutcome::Success
                } else {
                    queue.remove(0)
                };
                async move { next }
            },
        )
        .clock(self.time.clock())
        .sleeper(Arc::new(self.time.clone()))
        .jitter(Arc::new(move || *jitter.lock().unwrap()) as Arc<dyn JitterSource>)
        .build()
    }
}

/// The backoff starts at zero.
#[test]
fn test_backoff_starts_at_zero() {
    assert_eq!(BackoffTimer::default().current(), Duration::ZERO);
}

/// 30, 60, 120 for the polling schedule.
#[test]
fn test_backoff_doubles_geometrically() {
    let mut b = BackoffTimer::default();
    let seen: Vec<_> = (0..3)
        .map(|_| {
            b.record_failure();
            b.current()
        })
        .collect();
    assert_eq!(seen, [secs(30), secs(60), secs(120)]);
}

/// Ten or more failures give 600 and stay there.
#[test]
fn test_backoff_caps_at_maximum() {
    let mut b = BackoffTimer::default();
    for _ in 0..40 {
        b.record_failure();
    }
    assert_eq!(b.current(), secs(600));
    b.record_failure();
    assert_eq!(b.current(), secs(600));
}

/// (15, 300): 15, 30, then reset to 0.
#[test]
fn test_backoff_resets_on_success() {
    let mut b = BackoffTimer::new(secs(15), secs(300));
    b.record_failure();
    assert_eq!(b.current(), secs(15));
    b.record_failure();
    assert_eq!(b.current(), secs(30));
    b.reset();
    assert_eq!(b.current(), Duration::ZERO);
}

/// start runs one refresh then sleeps 300; firing runs refresh 2 then sleeps 300.
#[tokio::test(flavor = "current_thread")]
async fn test_refreshes_immediately_then_sleeps_the_full_interval() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(300));
    assert_eq!(h.count(), 1);
    assert_eq!(h.time.pending_count(), 1);
    h.time.fire();
    assert_eq!(h.time.next_sleep().await, secs(300));
    assert_eq!(h.count(), 2);
    l.stop();
}

/// nil: no refresh and no pending timer; set 60 + wake refreshes and sleeps 60;
/// nil + wake cancels the pending wait.
#[tokio::test(flavor = "current_thread")]
async fn test_none_interval_parks_without_a_timer_until_woken() {
    let h = Harness::new();
    h.set_interval(None);
    let l = h.make();
    l.start();
    h.time.settle().await;
    assert_eq!((h.count(), h.time.pending_count()), (0, 0));
    assert_eq!(l.delay_until_due(), None);

    h.set_interval(Some(60));
    l.wake();
    assert_eq!(h.time.next_sleep().await, secs(60));
    assert_eq!(h.count(), 1);

    h.set_interval(None);
    l.wake();
    h.time.settle().await;
    assert_eq!(
        h.time.pending_count(),
        0,
        "parking cancels the pending wait"
    );
    l.stop();
}

/// A zero interval parks like `None`.
#[tokio::test(flavor = "current_thread")]
async fn test_zero_interval_parks() {
    let h = Harness::new();
    *h.interval.lock().unwrap() = Some(Duration::ZERO);
    let l = h.make();
    l.start();
    h.time.settle().await;
    assert_eq!((h.count(), h.time.pending_count()), (0, 0));
}

/// After 100 s, interval 60 + wake: immediate refresh, sleep 60, one pending.
#[tokio::test(flavor = "current_thread")]
async fn test_wake_with_shorter_interval_refreshes_when_overdue() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    h.time.advance(secs(100));
    h.set_interval(Some(60));
    l.wake();
    assert_eq!(h.time.next_sleep().await, secs(60));
    assert_eq!(h.count(), 2);
    assert_eq!(h.time.pending_count(), 1);
    l.stop();
}

/// After 20 s, interval 60 + wake: sleep 40, no refresh.
#[tokio::test(flavor = "current_thread")]
async fn test_wake_rearms_the_remaining_wait_when_not_yet_due() {
    let h = Harness::new();
    h.set_interval(Some(300));
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    h.time.advance(secs(20));
    h.set_interval(Some(60));
    l.wake();
    assert_eq!(h.time.next_sleep().await, secs(40));
    assert_eq!(h.count(), 1);
    l.stop();
}

/// After 250 s, refresh_now refreshes and the next sleep is the full 300.
#[tokio::test(flavor = "current_thread")]
async fn test_refresh_now_restarts_the_wait_from_that_refresh() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    h.time.advance(secs(250));
    l.refresh_now().await;
    assert_eq!(h.count(), 2);
    assert_eq!(h.time.next_sleep().await, secs(300));
    l.stop();
}

/// `refresh_now_if_older_than` skips a fresh refresh and runs a stale one.
#[tokio::test(flavor = "current_thread")]
async fn test_refresh_now_if_older_than() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    h.time.advance(secs(10));
    l.refresh_now_if_older_than(secs(60)).await;
    assert_eq!(h.count(), 1);
    h.time.advance(secs(60));
    l.refresh_now_if_older_than(secs(60)).await;
    assert_eq!(h.count(), 2);
    l.stop();
}

/// Interval 10, failures: sleeps 30, 60, then 10 after a success.
#[tokio::test(flavor = "current_thread")]
async fn test_failures_back_off_exponentially_and_success_resets() {
    let h = Harness::new();
    h.set_interval(Some(10));
    let fail = PollOutcome::Failure { retry_after: None };
    *h.outcomes.lock().unwrap() = vec![fail, fail, PollOutcome::Success];
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(30));
    h.time.fire();
    assert_eq!(h.time.next_sleep().await, secs(60));
    h.time.fire();
    assert_eq!(h.time.next_sleep().await, secs(10));
    l.stop();
}

/// failure(retry_after 500) gives a sleep of 500, unjittered.
#[tokio::test(flavor = "current_thread")]
async fn test_retry_after_is_honoured_without_jitter() {
    let h = Harness::new();
    h.set_interval(Some(10));
    *h.jitter.lock().unwrap() = 0.1;
    *h.outcomes.lock().unwrap() = vec![PollOutcome::Failure {
        retry_after: Some(secs(500)),
    }];
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(500));
    l.stop();
}

/// A skipped refresh keeps the plain interval.
#[tokio::test(flavor = "current_thread")]
async fn test_skipped_refresh_keeps_the_interval() {
    let h = Harness::new();
    h.set_interval(Some(10));
    *h.outcomes.lock().unwrap() = vec![PollOutcome::Skipped];
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(10));
    l.stop();
}

/// Jitter 0.1 gives 330; -0.5 is clamped to -0.1 and gives 270.
#[tokio::test(flavor = "current_thread")]
async fn test_jitter_stretches_the_wait_and_is_clamped_to_ten_percent() {
    let h = Harness::new();
    *h.jitter.lock().unwrap() = 0.1;
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(330));
    l.stop();

    let h = Harness::new();
    *h.jitter.lock().unwrap() = -0.5;
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(270));
    l.stop();
}

/// A NaN jitter draw is treated as no jitter instead of poisoning the wait.
#[tokio::test(flavor = "current_thread")]
async fn test_nan_jitter_is_ignored() {
    let h = Harness::new();
    *h.jitter.lock().unwrap() = f64::NAN;
    let l = h.make();
    l.start();
    assert_eq!(h.time.next_sleep().await, secs(300));
    l.stop();
}

/// Stop cancels the pending wait: none pending, one cancelled, one refresh.
#[tokio::test(flavor = "current_thread")]
async fn test_stop_cancels_the_pending_wait() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    l.stop();
    h.time.settle().await;
    assert_eq!(h.time.pending_count(), 0);
    assert_eq!(h.time.cancelled_count(), 1);
    assert_eq!(h.count(), 1);
    h.time.advance(secs(10_000));
    h.time.fire();
    h.time.settle().await;
    assert_eq!(h.count(), 1);
}

/// Pause gives no pending wait; resume after 1000 s refreshes at once.
#[tokio::test(flavor = "current_thread")]
async fn test_pause_cancels_the_wait_and_resume_refreshes_when_overdue() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    l.pause();
    h.time.settle().await;
    assert_eq!(h.time.pending_count(), 0);
    // wake() while paused must not re-arm.
    l.wake();
    h.time.settle().await;
    assert_eq!(h.time.pending_count(), 0);
    h.time.advance(secs(1000));
    l.resume();
    assert_eq!(h.time.next_sleep().await, secs(300));
    assert_eq!(h.count(), 2);
    l.stop();
}

/// Resuming before the interval is overdue re-arms only the remainder.
#[tokio::test(flavor = "current_thread")]
async fn test_resume_when_not_overdue_waits_the_remainder() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    l.pause();
    h.time.advance(secs(100));
    l.resume();
    assert_eq!(h.time.next_sleep().await, secs(200));
    assert_eq!(h.count(), 1);
    l.stop();
}

/// A sleeping loop is not retained by its wait task; dropping it cancels the wait.
#[tokio::test(flavor = "current_thread")]
async fn test_drop_cancels_the_wait() {
    let h = Harness::new();
    let l = h.make();
    l.start();
    h.time.next_sleep().await;
    let weak = Arc::downgrade(&l);
    drop(l);
    h.time.settle().await;
    assert!(weak.upgrade().is_none());
    assert_eq!(h.time.pending_count(), 0);
    assert_eq!(h.time.cancelled_count(), 1);
}

/// stop() during an in-flight refresh lets it finish but schedules nothing.
#[tokio::test(flavor = "current_thread")]
async fn test_stop_during_refresh_does_not_abort_it_or_rearm() {
    let time = ManualScheduler::new();
    let gate = Arc::new(tokio::sync::Notify::new());
    let done = Arc::new(AtomicUsize::new(0));
    let (g, d) = (Arc::clone(&gate), Arc::clone(&done));
    let l = PollingLoop::builder(
        || Some(secs(60)),
        move || {
            let (g, d) = (Arc::clone(&g), Arc::clone(&d));
            async move {
                g.notified().await;
                d.fetch_add(1, Ordering::SeqCst);
                PollOutcome::Success
            }
        },
    )
    .clock(time.clock())
    .sleeper(Arc::new(time.clone()))
    .jitter(Arc::new(|| 0.0))
    .build();
    l.start();
    time.settle().await;
    assert!(l.is_refreshing());
    l.stop();
    l.wake(); // no-op while refreshing
    gate.notify_one();
    time.settle().await;
    assert_eq!(done.load(Ordering::SeqCst), 1);
    assert!(!l.is_refreshing());
    assert_eq!(time.pending_count(), 0);
}

/// refresh_now runs when not started but schedules nothing afterwards.
#[tokio::test(flavor = "current_thread")]
async fn test_refresh_now_without_start_schedules_nothing() {
    let h = Harness::new();
    let l = h.make();
    l.refresh_now().await;
    h.time.settle().await;
    assert_eq!(h.count(), 1);
    assert_eq!(h.time.pending_count(), 0);
}

/// Cancellation gives skipped; rate limiting gives failure(42); network gives
/// failure(None).
#[test]
fn test_outcome_classifies_cancellation_and_rate_limits() {
    assert_eq!(
        PollOutcome::from(&RefreshError::Cancelled),
        PollOutcome::Skipped
    );
    let limited = ProviderError::from_usage(
        UsageError::RateLimited {
            retry_after: Some(42.0),
        },
        ProviderErrorContext::Claude,
    );
    assert_eq!(
        PollOutcome::from(&limited),
        PollOutcome::Failure {
            retry_after: Some(secs(42))
        }
    );
    let net = ProviderError::Network(ProviderErrorContext::Claude, "down".into());
    assert_eq!(
        PollOutcome::from(&net),
        PollOutcome::Failure { retry_after: None }
    );
}

/// The real sleeper under tokio's paused time drives a loop with no real waiting.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_tokio_sleeper_with_paused_time() {
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let l = PollingLoop::builder_unit(
        || Some(secs(60)),
        move || {
            c.fetch_add(1, Ordering::SeqCst);
            async {}
        },
    )
    .jitter(Arc::new(|| 0.0))
    .build();
    l.start();
    tokio::time::sleep(secs(1)).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    tokio::time::sleep(secs(60)).await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
    l.stop();
    tokio::time::sleep(secs(600)).await;
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

/// The real sleeper clamps a huge wait to one day.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_tokio_sleeper_clamps_to_one_day() {
    let start = tokio::time::Instant::now();
    TokioSleeper.sleep(Duration::from_secs(10_000_000)).await;
    assert_eq!(start.elapsed(), secs(86_400));
}

/// The default random jitter stays within +/-10%.
#[test]
fn test_random_jitter_is_bounded() {
    use tokenmon_core::polling::RandomJitter;
    let j = RandomJitter::with_seed(1);
    assert!((0..10_000).all(|_| j.next_fraction().abs() <= 0.1));
}
