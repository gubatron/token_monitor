//! The refresh loop (spec 01 section 1.2.3).

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use super::{BackoffTimer, JitterSource, PollOutcome, RandomJitter, Sleeper, TokioSleeper};
use crate::clock::{Clock, SystemClock};
use crate::provider::BoxFuture;

/// Largest jitter fraction either side of the plain wait.
const MAX_JITTER: f64 = 0.1;
/// Waits are clamped here so date arithmetic can never overflow (about 31 years).
const MAX_WAIT_SECS: f64 = 1.0e9;

type IntervalFn = Arc<dyn Fn() -> Option<Duration> + Send + Sync>;
type RefreshFn = Arc<dyn Fn() -> BoxFuture<'static, PollOutcome> + Send + Sync>;

/// Mutable loop state, guarded by one lock so transitions are atomic.
struct State {
    started: bool,
    paused: bool,
    refreshing: bool,
    last_refresh_at: Option<DateTime<Utc>>,
    backoff: BackoffTimer,
    retry_after_until: Option<DateTime<Utc>>,
    jitter_factor: f64,
    /// Bumped on every re-arm and cancel so a stale wait task never refreshes.
    epoch: u64,
    wait: Option<JoinHandle<()>>,
}

/// Refresh loop for one provider.
///
/// Refreshes as soon as it starts, then waits the full interval between
/// refreshes. An `interval` of `None` (or zero) **parks** the loop: no timer at
/// all until [`wake`](Self::wake) re-evaluates it. Callers wake the loop when an
/// input to `interval` changes (menu opened, settings edited); that cancels the
/// pending wait and re-arms it from the last refresh, refreshing at once when
/// that is already overdue.
///
/// After a failed refresh the wait grows exponentially ([`BackoffTimer`]) and a
/// server `Retry-After` is honoured as an absolute deadline with no jitter.
/// Every other wait carries +/-10% jitter so loops started together drift apart.
///
/// Waits are spawned on the tokio runtime captured when the loop was built, so
/// every method may be called from any thread (for example the UI thread). A
/// sleeping loop is not kept alive by its wait task: dropping the last `Arc`
/// cancels the wait. A refresh already in flight is never aborted by `stop`,
/// `pause` or `wake`; it finishes and the next wait is then computed.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use tokenmon_core::polling::{PollOutcome, PollingLoop};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let parked = PollingLoop::builder(|| None, || async { PollOutcome::Success }).build();
/// parked.start();
/// assert_eq!(parked.delay_until_due(), None); // parked: no timer
/// # });
/// ```
pub struct PollingLoop {
    interval: IntervalFn,
    refresh: RefreshFn,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    jitter: Arc<dyn JitterSource>,
    handle: Handle,
    weak_self: Weak<Self>,
    state: Mutex<State>,
}

/// Builder for [`PollingLoop`]; every injected dependency has a real default.
pub struct PollingLoopBuilder {
    interval: IntervalFn,
    refresh: RefreshFn,
    backoff: BackoffTimer,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    jitter: Arc<dyn JitterSource>,
    handle: Option<Handle>,
}

impl PollingLoopBuilder {
    /// Overrides the failure backoff (default 30 s doubling to 600 s).
    #[must_use]
    pub fn backoff(mut self, backoff: BackoffTimer) -> Self {
        self.backoff = backoff;
        self
    }

    /// Overrides the clock (tests inject a `ManualClock`).
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Overrides the sleeper (tests inject a `ManualScheduler`).
    #[must_use]
    pub fn sleeper(mut self, sleeper: Arc<dyn Sleeper>) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// Overrides the jitter source (default uniform in `-0.1..0.1`).
    #[must_use]
    pub fn jitter(mut self, jitter: Arc<dyn JitterSource>) -> Self {
        self.jitter = jitter;
        self
    }

    /// Runs waits on `handle` instead of the runtime current at `build`.
    #[must_use]
    pub fn handle(mut self, handle: Handle) -> Self {
        self.handle = Some(handle);
        self
    }

    /// Builds the (not yet started) loop.
    ///
    /// # Panics
    /// If no [`handle`](Self::handle) was given and this is called outside a
    /// tokio runtime.
    pub fn build(self) -> Arc<PollingLoop> {
        let handle = self.handle.unwrap_or_else(Handle::current);
        Arc::new_cyclic(|weak| PollingLoop {
            interval: self.interval,
            refresh: self.refresh,
            clock: self.clock,
            sleeper: self.sleeper,
            jitter: self.jitter,
            handle,
            weak_self: weak.clone(),
            state: Mutex::new(State {
                started: false,
                paused: false,
                refreshing: false,
                last_refresh_at: None,
                backoff: self.backoff,
                retry_after_until: None,
                jitter_factor: 1.0,
                epoch: 0,
                wait: None,
            }),
        })
    }
}

/// Clears `refreshing` if the refresh future panics or is dropped.
struct RefreshingGuard<'a> {
    owner: &'a PollingLoop,
    armed: bool,
}

impl Drop for RefreshingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.owner.lock().refreshing = false;
        }
    }
}

impl PollingLoop {
    /// Starts a builder.
    ///
    /// `interval` returns the wait between refreshes, or `None` to park.
    /// `refresh` performs one poll and reports its [`PollOutcome`].
    pub fn builder<I, R, Fut>(interval: I, refresh: R) -> PollingLoopBuilder
    where
        I: Fn() -> Option<Duration> + Send + Sync + 'static,
        R: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = PollOutcome> + Send + 'static,
    {
        PollingLoopBuilder {
            interval: Arc::new(interval),
            refresh: Arc::new(move || Box::pin(refresh())),
            backoff: BackoffTimer::default(),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(TokioSleeper),
            jitter: Arc::new(RandomJitter::from_entropy()),
            handle: None,
        }
    }

    /// Starts a builder whose refreshes report no outcome, so the loop always
    /// waits the plain interval (the update checker).
    pub fn builder_unit<I, R, Fut>(interval: I, refresh: R) -> PollingLoopBuilder
    where
        I: Fn() -> Option<Duration> + Send + Sync + 'static,
        R: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self::builder(interval, move || {
            let fut = refresh();
            async move {
                fut.await;
                PollOutcome::Success
            }
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts polling: refreshes now when `interval()` is set, else parks.
    /// Forgets the last refresh time.
    pub fn start(&self) {
        {
            let mut s = self.lock();
            s.started = true;
            s.last_refresh_at = None;
        }
        self.schedule();
    }

    /// Stops polling and cancels the pending wait.
    pub fn stop(&self) {
        self.lock().started = false;
        self.cancel_wait();
    }

    /// Suspends the timer (system sleep) without forgetting the last refresh.
    pub fn pause(&self) {
        self.lock().paused = true;
        self.cancel_wait();
    }

    /// Resumes after [`pause`](Self::pause), refreshing at once when overdue.
    pub fn resume(&self) {
        self.lock().paused = false;
        self.schedule();
    }

    /// Re-evaluates `interval()` and re-arms the wait from the last refresh.
    /// A no-op while a refresh is in flight (the next wait is computed after it).
    pub fn wake(&self) {
        if self.is_refreshing() {
            return;
        }
        self.schedule();
    }

    /// Refreshes immediately and restarts the wait from this refresh. A no-op
    /// while a refresh is already in flight. Runs even when not started, but
    /// then schedules nothing afterwards.
    pub async fn refresh_now(&self) {
        if self.is_refreshing() {
            return;
        }
        self.cancel_wait();
        self.run_refresh().await;
    }

    /// As [`refresh_now`](Self::refresh_now), but only when the last refresh is
    /// older than `age` (or there has been none).
    pub async fn refresh_now_if_older_than(&self, age: Duration) {
        let last = self.lock().last_refresh_at;
        if let Some(last) = last {
            let elapsed = (self.clock.now() - last).to_std().unwrap_or(Duration::ZERO);
            if elapsed < age {
                return;
            }
        }
        self.refresh_now().await;
    }

    /// Whether a refresh is currently in flight.
    pub fn is_refreshing(&self) -> bool {
        self.lock().refreshing
    }

    /// When the last refresh finished, if any.
    pub fn last_refresh_at(&self) -> Option<DateTime<Utc>> {
        self.lock().last_refresh_at
    }

    /// Time until the next refresh is due, or `None` when parked.
    ///
    /// `Some(ZERO)` means due now. The wait is `max(interval, backoff)` times
    /// the jitter factor from the last refresh, or the `Retry-After` deadline
    /// if that is later.
    pub fn delay_until_due(&self) -> Option<Duration> {
        let base = (self.interval)().filter(|d| !d.is_zero())?;
        let now = self.clock.now();
        let s = self.lock();
        let Some(last) = s.last_refresh_at else {
            return Some(Duration::ZERO);
        };
        let wait = base.as_secs_f64().max(s.backoff.current().as_secs_f64());
        let jittered = (wait * s.jitter_factor).clamp(0.0, MAX_WAIT_SECS);
        let mut due = add_secs(last, jittered);
        if let Some(until) = s.retry_after_until
            && until > due
        {
            due = until;
        }
        Some((due - now).to_std().unwrap_or(Duration::ZERO))
    }

    fn schedule(&self) {
        self.cancel_wait();
        // Evaluate the caller's closure before taking the lock.
        let Some(delay) = self.delay_until_due() else {
            return;
        };
        let mut s = self.lock();
        if !s.started || s.paused {
            return;
        }
        s.epoch += 1;
        let epoch = s.epoch;
        let weak = self.weak_self.clone();
        let sleep = (!delay.is_zero()).then(|| self.sleeper.sleep(delay));
        s.wait = Some(self.handle.spawn(async move {
            if let Some(sleep) = sleep {
                sleep.await;
            }
            let Some(this) = weak.upgrade() else { return };
            if this.claim(epoch) {
                this.run_refresh_claimed().await;
            }
        }));
    }

    /// Marks a refresh as starting if this wait is still the current one.
    fn claim(&self, epoch: u64) -> bool {
        let mut s = self.lock();
        if s.epoch != epoch || s.refreshing || !s.started || s.paused {
            return false;
        }
        s.refreshing = true;
        true
    }

    fn cancel_wait(&self) {
        let mut s = self.lock();
        s.epoch += 1;
        if let Some(wait) = s.wait.take()
            && !s.refreshing
        {
            wait.abort();
        }
    }

    async fn run_refresh(&self) {
        {
            let mut s = self.lock();
            if s.refreshing {
                return;
            }
            s.refreshing = true;
        }
        self.run_refresh_claimed().await;
    }

    /// Runs one refresh; the caller has already set `refreshing`.
    async fn run_refresh_claimed(&self) {
        let mut guard = RefreshingGuard {
            owner: self,
            armed: true,
        };
        // A panicking refresh (debug builds; release aborts) is a failure like
        // any other: it backs off and the loop re-arms instead of staying dead
        // until the next wake. The call itself is inside the async block so a
        // synchronous panic in the closure is caught too.
        let result = AssertUnwindSafe(async { (self.refresh)().await })
            .catch_unwind()
            .await;
        let outcome = result.unwrap_or_else(|_| {
            tracing::error!("provider refresh panicked; backing off and rescheduling");
            PollOutcome::Failure { retry_after: None }
        });
        self.record(outcome);
        guard.armed = false;
        self.schedule();
    }

    fn record(&self, outcome: PollOutcome) {
        let finished_at = self.clock.now();
        let fraction = self.jitter.next_fraction();
        let fraction = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(-MAX_JITTER, MAX_JITTER)
        };
        let mut s = self.lock();
        s.refreshing = false;
        s.last_refresh_at = Some(finished_at);
        s.jitter_factor = 1.0 + fraction;
        match outcome {
            PollOutcome::Success => {
                s.backoff.reset();
                s.retry_after_until = None;
            }
            PollOutcome::Failure { retry_after } => {
                s.backoff.record_failure();
                s.retry_after_until =
                    retry_after.map(|d| add_secs(finished_at, d.as_secs_f64().min(MAX_WAIT_SECS)));
            }
            PollOutcome::Skipped => {}
        }
    }
}

impl Drop for PollingLoop {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(wait) = state.wait.take()
            && !state.refreshing
        {
            wait.abort();
        }
    }
}

/// `at + secs`, saturating instead of panicking on overflow.
fn add_secs(at: DateTime<Utc>, secs: f64) -> DateTime<Utc> {
    let nanos = (secs * 1.0e9) as i64;
    at.checked_add_signed(chrono::Duration::nanoseconds(nanos))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}
