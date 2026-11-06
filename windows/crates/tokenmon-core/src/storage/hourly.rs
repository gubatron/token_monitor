//! `HourlyDeltaActivityStore` (spec 1.3.4): percentage-point growth per local hour today.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::calendar::{
    day_key, from_apple_seconds, key_for_stored_start_of_day, local_hour, shift_instant_days,
    start_of_day, to_apple_seconds,
};
use super::delta::{NOISE_FLOOR, credited_delta};
use super::numfmt::Num;
use super::string_store::TextStore;
use crate::clock::Clock;

/// Storage key prefix semantics: stores are created over a `activity_` prefixed
/// [`TextStore`]; this is the number of hours in a day.
const HOURS: usize = 24;

/// The persisted shape: `{"dayKey":..,"dayStart":<ref-secs>,"hourWeights":[24],"lastUsedPercent":..}`.
///
/// Optionals are omitted when `None` (as Swift's `JSONEncoder` does); `dayKey`
/// may be absent in old payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HourlyPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    day_key: Option<String>,
    day_start: Num,
    hour_weights: Vec<Num>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_used_percent: Option<Num>,
}

/// Decodes a persisted hourly payload; `None` when it is not valid.
///
/// Total: never panics on any input.
///
/// # Examples
///
/// ```
/// use tokenmon_core::storage::decode_hourly_payload;
/// let raw = r#"{"dayKey":"2026-10-10","dayStart":813304800,"hourWeights":[0,0],"lastUsedPercent":30}"#;
/// let payload = decode_hourly_payload(raw).unwrap();
/// assert_eq!(serde_json::to_string(&payload).unwrap(), raw);
/// assert!(decode_hourly_payload("not json").is_none());
/// ```
pub fn decode_hourly_payload(raw: &str) -> Option<HourlyPayload> {
    serde_json::from_str(raw).ok()
}

/// Tracks one provider's percentage-point growth per local hour today.
///
/// Persists through a [`TextStore`] (callers pass a `FileBackedStringStore` with
/// prefix `activity_`) under `storage_key`, e.g. `grok_hourly_today`.
/// Callers needing shared access wrap the store in a mutex.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::{ManualClock};
/// use tokenmon_core::storage::{HourlyDeltaActivityStore, InMemoryStringStore};
/// let t = Utc.with_ymd_and_hms(2026, 10, 10, 9, 0, 0).unwrap();
/// let clock = Arc::new(ManualClock::new(t));
/// let mut store = HourlyDeltaActivityStore::new(Arc::new(InMemoryStringStore::default()), "k", clock);
/// store.record(10.0, t);
/// store.record(25.0, t);
/// assert_eq!(store.hour_weights()[9], 15.0);
/// ```
pub struct HourlyDeltaActivityStore {
    store: Arc<dyn TextStore>,
    storage_key: String,
    clock: Arc<dyn Clock>,
    hour_weights: [f64; HOURS],
    day_key: String,
    last_used_percent: Option<f64>,
    persisted: Option<HourlyPayload>,
}

impl HourlyDeltaActivityStore {
    /// Loads the persisted day if it is today's, else starts an empty day.
    pub fn new(
        store: Arc<dyn TextStore>,
        storage_key: impl Into<String>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let now = clock.now();
        let mut this = Self {
            store,
            storage_key: storage_key.into(),
            day_key: day_key(clock.as_ref(), now),
            clock,
            hour_weights: [0.0; HOURS],
            last_used_percent: None,
            persisted: None,
        };
        this.load_or_reset();
        this
    }

    /// The 24 per-hour weights (percentage points) for the current day.
    pub fn hour_weights(&self) -> [f64; HOURS] {
        self.hour_weights
    }

    /// Local midnight of the current day, in the clock's current zone.
    pub fn day_start(&self) -> DateTime<Utc> {
        start_of_day(self.clock.as_ref(), &self.day_key).unwrap_or_else(|| self.clock.now())
    }

    /// The `yyyy-MM-dd` key of the current day.
    pub fn day_key(&self) -> &str {
        &self.day_key
    }

    /// The baseline the next sample is compared with.
    pub fn last_used_percent(&self) -> Option<f64> {
        self.last_used_percent
    }

    /// Records a usage sample taken at `at`, crediting growth to its local hour.
    ///
    /// The first sample only sets the baseline. A drop of 5+ points is a window
    /// reset and credits the post-reset value; smaller drops are noise. Deltas
    /// under 0.05 are ignored. Non-finite samples are ignored.
    pub fn record(&mut self, used_percent: f64, at: DateTime<Utc>) {
        if !used_percent.is_finite() {
            return;
        }
        self.roll_day_if_needed(at);
        let previous = self.last_used_percent.replace(used_percent);
        if let Some(previous) = previous {
            let delta = credited_delta(used_percent, previous);
            if delta >= NOISE_FLOOR {
                self.hour_weights[local_hour(self.clock.as_ref(), at).min(HOURS - 1)] += delta;
            }
        }
        self.persist();
    }

    /// Clears the series and baseline (account switch).
    pub fn clear(&mut self, at: DateTime<Utc>) {
        self.day_key = day_key(self.clock.as_ref(), at);
        self.hour_weights = [0.0; HOURS];
        self.last_used_percent = None;
        self.persist();
    }

    /// Starts a new quota window: zeroes the hours unless `keeping_hours`, and
    /// sets the baseline to 0 so the first sample of the window is credited whole.
    pub fn begin_new_window(&mut self, keeping_hours: bool) {
        if !keeping_hours {
            self.hour_weights = [0.0; HOURS];
        }
        self.last_used_percent = Some(0.0);
        self.persist();
    }

    fn roll_day_if_needed(&mut self, at: DateTime<Utc>) {
        let key = day_key(self.clock.as_ref(), at);
        if key == self.day_key {
            return;
        }
        let yesterday = shift_instant_days(self.clock.as_ref(), at, -1)
            .map(|y| day_key(self.clock.as_ref(), y));
        if yesterday.as_deref() != Some(self.day_key.as_str()) {
            self.last_used_percent = None;
        }
        self.day_key = key;
        self.hour_weights = [0.0; HOURS];
    }

    fn load_or_reset(&mut self) {
        let payload = self
            .store
            .value(&self.storage_key)
            .and_then(|raw| decode_hourly_payload(&raw));
        let Some(payload) = payload else {
            self.persist();
            return;
        };
        let stored_key = payload
            .day_key
            .clone()
            .or_else(|| self.legacy_key(&payload));
        if stored_key.as_deref() == Some(self.day_key.as_str())
            && payload.hour_weights.len() == HOURS
        {
            for (slot, weight) in self.hour_weights.iter_mut().zip(&payload.hour_weights) {
                *slot = weight.0;
            }
            self.last_used_percent = payload.last_used_percent.map(|n| n.0);
        }
        self.persisted = Some(payload);
        self.persist();
    }

    fn legacy_key(&self, payload: &HourlyPayload) -> Option<String> {
        let instant = from_apple_seconds(payload.day_start.0)?;
        let offset = self.clock.local_offset_at(instant).local_minus_utc();
        Some(key_for_stored_start_of_day(instant, offset))
    }

    fn payload(&self) -> HourlyPayload {
        HourlyPayload {
            day_key: Some(self.day_key.clone()),
            day_start: Num(to_apple_seconds(self.day_start())),
            hour_weights: self.hour_weights.iter().copied().map(Num).collect(),
            last_used_percent: self.last_used_percent.map(Num),
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
