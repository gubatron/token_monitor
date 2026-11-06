//! `ObservationLoop` semantics (the Swift `ObservationLoopTests` rows).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokenmon_app::observe::ObservationLoop;
use tokio::runtime::Handle;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default)]
struct Counter {
    value: i32,
    unrelated: i32,
}

type Seen = Arc<Mutex<Vec<i32>>>;

fn start(
    tx: &watch::Sender<Counter>,
    coalescing: Duration,
    initial: bool,
) -> (ObservationLoop, Seen) {
    let seen: Seen = Arc::default();
    let sink = Arc::clone(&seen);
    let l = ObservationLoop::start(
        &Handle::current(),
        tx.subscribe(),
        coalescing,
        initial,
        |c: &Counter| c.value,
        move |v| sink.lock().expect("lock").push(v),
    );
    (l, seen)
}

async fn settle() {
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
}

fn seen(s: &Seen) -> Vec<i32> {
    s.lock().expect("lock").clone()
}

/// The first reading is handled, then each change; a value the projection did
/// not read is not a dependency.
#[tokio::test]
async fn test_handles_the_first_reading_and_each_change() {
    let (tx, _rx) = watch::channel(Counter::default());
    let (l, s) = start(&tx, Duration::ZERO, true);
    settle().await;
    assert_eq!(seen(&s), [0]);
    tx.send_modify(|c| c.value = 1);
    settle().await;
    assert_eq!(seen(&s), [0, 1]);
    tx.send_modify(|c| c.unrelated = 5);
    settle().await;
    assert_eq!(seen(&s), [0, 1], "unrelated is not a dependency");
    l.cancel();
}

/// A burst inside the coalescing window is handled once with the latest value.
#[tokio::test]
async fn test_burst_is_handled_once_with_the_latest_value() {
    let (tx, _rx) = watch::channel(Counter::default());
    let (l, s) = start(&tx, Duration::from_millis(30), false);
    for v in 1..=3 {
        tx.send_modify(|c| c.value = v);
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(seen(&s), [3]);
    l.cancel();
}

/// After cancel nothing is handled.
#[tokio::test]
async fn test_cancel_stops_handling() {
    let (tx, _rx) = watch::channel(Counter::default());
    let (l, s) = start(&tx, Duration::ZERO, false);
    l.cancel();
    tx.send_modify(|c| c.value = 1);
    settle().await;
    assert!(seen(&s).is_empty());
}
