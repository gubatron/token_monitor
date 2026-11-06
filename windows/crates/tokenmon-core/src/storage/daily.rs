//! `DailyQuotaDeltaStore` (spec 1.3.5): percentage-point growth per local calendar day.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::calendar::{
    add_days_to_key, day_key, from_apple_seconds, key_for_stored_start_of_day, shift_instant_days,
    start_of_day, to_apple_seconds,
};
use super::delta::{NOISE_FLOOR, credited_delta};
use super::numfmt::Num;
use super::quota::{QuotaWindow, QuotaWindowTransition};
use super::string_store::TextStore;
use crate::clock::Clock;

/// How many days of history are kept.
pub const RETENTION_DAYS: i64 = 40;

/// The persisted shape. Both day encodings are written; `days` is the macOS
/// flat `[ref-seconds, value, ...]` array and is only read when `dayTotals` is absent.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    day_totals: Option<BTreeMap<String, Num>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    days: Option<Vec<Num>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_used_percent: Option<Num>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_start: Option<Num>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    interrupted_window_start: Option<Num>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_start: Option<Num>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_resets_at: Option<Num>,
}

/// Decodes a persisted daily payload; `None` when it is not valid.
///
/// Total: never panics on any input.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::decode_daily_payload;
/// assert!(decode_daily_payload(r#"{"days":[811058400,7],"lastUsedPercent":35}"#).is_some());
/// assert!(decode_daily_payload("42").is_none());
/// ```
pub fn decode_daily_payload(raw: &str) -> Option<DailyPayload> {
    serde_json::from_str(raw).ok()
}

/// Accumulates percentage points per local calendar day, with quota-window tracking.
///
/// Persists under `storage_key` (e.g. `claude_daily_usage`) in a [`TextStore`].
/// Callers needing shared access wrap the store in a mutex.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_core::storage::{DailyQuotaDeltaStore, InMemoryStringStore};
/// let t = Utc.with_ymd_and_hms(2026, 10, 10, 9, 0, 0).unwrap();
/// let clock = Arc::new(ManualClock::new(t));
/// let mut store = DailyQuotaDeltaStore::new(Arc::new(InMemoryStringStore::default()), "k", clock);
/// store.record(10.0, t, None, 7);
/// store.record(25.0, t, None, 7);
/// assert_eq!(store.day_totals()["2026-10-10"], 15.0);
/// ```
pub struct DailyQuotaDeltaStore {
    store: Arc<dyn TextStore>,
    storage_key: String,
    clock: Arc<dyn Clock>,
    day_totals: BTreeMap<String, f64>,
    last_used_percent: Option<f64>,
    window_start: Option<DateTime<Utc>>,
    interrupted_window_start: Option<DateTime<Utc>>,
    observed_start: Option<DateTime<Utc>>,
    window_resets_at: Option<DateTime<Utc>>,
    persisted: Option<DailyPayload>,
}

impl DailyQuotaDeltaStore {
    /// Loads persisted state (rewriting legacy payloads once), or starts empty.
    pub fn new(
        store: Arc<dyn TextStore>,
        storage_key: impl Into<String>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let mut this = Self {
            store,
            storage_key: storage_key.into(),
            clock,
            day_totals: BTreeMap::new(),
            last_used_percent: None,
            window_start: None,
            interrupted_window_start: None,
            observed_start: None,
            window_resets_at: None,
            persisted: None,
        };
        this.load();
        this
    }

    /// Per-day totals keyed by `yyyy-MM-dd`.
    pub fn day_totals(&self) -> &BTreeMap<String, f64> {
        &self.day_totals
    }

    /// Totals keyed by the local start-of-day instant, in the clock's current zone.
    ///
    /// Derived on every call, so a time-zone change is picked up automatically.
    /// Keys landing on the same instant are summed.
    pub fn spent_by_day(&self) -> BTreeMap<DateTime<Utc>, f64> {
        self.day_totals
            .iter()
            .filter_map(|(key, v)| Some((start_of_day(self.clock.as_ref(), key)?, *v)))
            .fold(BTreeMap::new(), |mut acc, (day, v)| {
                *acc.entry(day).or_insert(0.0) += v;
                acc
            })
    }

    /// The baseline the next sample is compared with.
    pub fn last_used_percent(&self) -> Option<f64> {
        self.last_used_percent
    }

    /// When the current window began (as reported or derived).
    pub fn window_start(&self) -> Option<DateTime<Utc>> {
        self.window_start
    }

    /// The start of the window that an early reset cut short.
    pub fn interrupted_window_start(&self) -> Option<DateTime<Utc>> {
        self.interrupted_window_start
    }

    /// The window start most recently reported by the provider.
    pub fn observed_start(&self) -> Option<DateTime<Utc>> {
        self.observed_start
    }

    /// The reset instant most recently reported by the provider.
    pub fn window_resets_at(&self) -> Option<DateTime<Utc>> {
        self.window_resets_at
    }

    /// Records a usage sample taken at `at`, crediting growth to its local day.
    ///
    /// When `window` is given it is classified against the previously observed
    /// window; a [`QuotaWindowTransition::Rollover`] or
    /// [`QuotaWindowTransition::EarlyReset`] starts a new window first. The
    /// transition is returned so the caller can reset its own per-window state.
    /// Non-finite samples are ignored.
    pub fn record(
        &mut self,
        window_used_percent: f64,
        at: DateTime<Utc>,
        window: Option<QuotaWindow>,
        period_days: i64,
    ) -> QuotaWindowTransition {
        if !window_used_percent.is_finite() {
            return QuotaWindowTransition::None;
        }
        let transition = self.record_inner(window_used_percent, at, window, period_days);
        self.persist();
        transition
    }

    fn record_inner(
        &mut self,
        used: f64,
        at: DateTime<Utc>,
        window: Option<QuotaWindow>,
        period_days: i64,
    ) -> QuotaWindowTransition {
        let transition = window.map_or(QuotaWindowTransition::None, |w| {
            let previous = QuotaWindow {
                start: self.observed_start,
                resets_at: self.window_resets_at,
            };
            QuotaWindowTransition::classify(
                previous,
                self.last_used_percent,
                w,
                used,
                period_days,
                at,
            )
        });
        if let Some(w) = window {
            if transition != QuotaWindowTransition::None {
                self.apply_transition(transition, w, period_days, at);
            }
            self.observed_start = w.start.or(self.observed_start);
            self.window_resets_at = w.resets_at.or(self.window_resets_at);
        }
        let previous = self.last_used_percent.replace(used);
        let Some(previous) = previous else {
            return transition;
        };
        let delta = credited_delta(used, previous);
        if delta >= NOISE_FLOOR {
            let key = day_key(self.clock.as_ref(), at);
            *self.day_totals.entry(key).or_insert(0.0) += delta;
            self.prune(at);
        }
        transition
    }

    fn implied_start(&self, reset: DateTime<Utc>, period_days: i64) -> Option<DateTime<Utc>> {
        shift_instant_days(self.clock.as_ref(), reset, -period_days.clamp(1, 36_500))
    }

    fn apply_transition(
        &mut self,
        transition: QuotaWindowTransition,
        window: QuotaWindow,
        period_days: i64,
        at: DateTime<Utc>,
    ) {
        let old_window_end = (transition == QuotaWindowTransition::Rollover)
            .then_some(self.window_resets_at)
            .flatten();
        let candidate = window.start.or(old_window_end).or_else(|| {
            window
                .resets_at
                .and_then(|r| self.implied_start(r, period_days))
        });
        let interrupted = (transition == QuotaWindowTransition::EarlyReset)
            .then(|| {
                self.observed_start.or_else(|| {
                    self.window_resets_at
                        .and_then(|r| self.implied_start(r, period_days))
                })
            })
            .flatten();
        let start = candidate.map_or(at, |c| c.min(at));
        self.begin_new_window(Some(start), interrupted);
    }

    /// Starts a new window at `starting_at` (default: now): drops the boundary
    /// day and later days, sets the baseline to 0 so the next sample is credited
    /// whole, and records the interrupted window's start (early resets).
    pub fn begin_new_window(
        &mut self,
        starting_at: Option<DateTime<Utc>>,
        interrupted_window_start: Option<DateTime<Utc>>,
    ) {
        let start = starting_at.unwrap_or_else(|| self.clock.now());
        let boundary = day_key(self.clock.as_ref(), start);
        self.day_totals
            .retain(|key, _| key.as_str() < boundary.as_str());
        self.last_used_percent = Some(0.0);
        self.window_start = Some(start);
        self.interrupted_window_start = interrupted_window_start;
        self.persist();
    }

    /// Forgets everything: window metadata, baseline and all days.
    pub fn clear(&mut self) {
        self.day_totals.clear();
        self.last_used_percent = None;
        self.window_start = None;
        self.interrupted_window_start = None;
        self.observed_start = None;
        self.window_resets_at = None;
        self.persist();
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let today = day_key(self.clock.as_ref(), now);
        if let Some(cutoff) = add_days_to_key(&today, -RETENTION_DAYS) {
            self.day_totals
                .retain(|key, _| key.as_str() >= cutoff.as_str());
        }
    }

    fn load(&mut self) {
        let payload = self
            .store
            .value(&self.storage_key)
            .and_then(|raw| decode_daily_payload(&raw));
        let Some(payload) = payload else {
            self.persist();
            return;
        };
        self.day_totals = match &payload.day_totals {
            Some(totals) => totals.iter().map(|(k, v)| (k.clone(), v.0)).collect(),
            None => self.convert_legacy_days(payload.days.as_deref().unwrap_or_default()),
        };
        let clock = Arc::clone(&self.clock);
        self.day_totals
            .retain(|key, _| start_of_day(clock.as_ref(), key).is_some());
        self.prune(clock.now());
        self.last_used_percent = payload.last_used_percent.map(|n| n.0);
        let date = |n: Option<Num>| n.and_then(|n| from_apple_seconds(n.0));
        self.window_start = date(payload.window_start);
        self.interrupted_window_start = date(payload.interrupted_window_start);
        self.observed_start = date(payload.observed_start);
        self.window_resets_at = date(payload.window_resets_at);
        self.persisted = Some(payload);
        self.persist();
    }

    fn convert_legacy_days(&self, flat: &[Num]) -> BTreeMap<String, f64> {
        flat.as_chunks::<2>()
            .0
            .iter()
            .filter_map(|pair| {
                let instant = from_apple_seconds(pair[0].0)?;
                let offset = self.clock.local_offset_at(instant).local_minus_utc();
                Some((key_for_stored_start_of_day(instant, offset), pair[1].0))
            })
            .fold(BTreeMap::new(), |mut acc, (key, v)| {
                *acc.entry(key).or_insert(0.0) += v;
                acc
            })
    }

    fn payload(&self) -> DailyPayload {
        let days: Vec<Num> = self
            .spent_by_day()
            .into_iter()
            .flat_map(|(day, v)| [Num(to_apple_seconds(day)), Num(v)])
            .collect();
        let date = |d: Option<DateTime<Utc>>| d.map(|d| Num(to_apple_seconds(d)));
        DailyPayload {
            day_totals: Some(
                self.day_totals
                    .iter()
                    .map(|(k, v)| (k.clone(), Num(*v)))
                    .collect(),
            ),
            days: Some(days),
            last_used_percent: self.last_used_percent.map(Num),
            window_start: date(self.window_start),
            interrupted_window_start: date(self.interrupted_window_start),
            observed_start: date(self.observed_start),
            window_resets_at: date(self.window_resets_at),
        }
    }

    fn persist(&mut self) {
        let payload = self.payload();
        if self.persisted.as_ref() == Some(&payload) {
            return;
        }
        let Ok(json) = serde_json::to_string(&payload) else {
            return;
        };
        if self.store.set(&json, &self.storage_key) {
            self.persisted = Some(payload);
        }
    }
}
