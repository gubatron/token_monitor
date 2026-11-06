//! `ThresholdNotifier` logic (spec 01 section 5), with no OS toast.
//!
//! [`ThresholdNotifier::evaluate`] decides whether a usage reading crosses the
//! user's threshold, remembers what it already announced (so an alert fires once
//! per crossing and again only after usage drops at least five points below the
//! recorded threshold or the billing period rolls over), and hands a
//! [`ThresholdAlert`] to a [`Notifier`]. The Windows toast implementation of
//! [`Notifier`] lives in `ws-notify`.
//!
//! Owner: ws-settings.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::provider::ProviderId;
use crate::settings::{Settings, SettingsFile};

/// Points below the recorded threshold usage must fall before the alert re-arms.
pub const REARM_DROP_POINTS: f64 = 5.0;
/// Slack (seconds) when deciding that a reset time belongs to a later period.
pub const PERIOD_SLACK_SECONDS: f64 = 3600.0;
/// Notification title.
pub const ALERT_TITLE: &str = "TokenMon Alert";

const NOTIFIED_PREFIX: &str = "thresholdNotified.";
const PERIOD_PREFIX: &str = "thresholdNotifiedResetsAt.";

/// A threshold crossing ready to be shown to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdAlert {
    /// The provider that crossed its threshold.
    pub provider: ProviderId,
    /// Usage at the time of the crossing, in percent.
    pub used_percent: f64,
    /// The configured threshold, in percent.
    pub threshold: f64,
}

impl ThresholdAlert {
    /// The notification title (`"TokenMon Alert"`).
    pub fn title(&self) -> &'static str {
        ALERT_TITLE
    }

    /// The notification body, e.g. `Weekly SuperGrok usage is at 85% (threshold 80%).`
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::ProviderId;
    /// use tokenmon_core::alerts::ThresholdAlert;
    ///
    /// let a = ThresholdAlert { provider: ProviderId::Grok, used_percent: 85.4, threshold: 80.0 };
    /// assert_eq!(a.body(), "Weekly SuperGrok usage is at 85% (threshold 80%).");
    /// ```
    pub fn body(&self) -> String {
        let subject = match self.provider {
            ProviderId::Grok => "Weekly SuperGrok usage".to_owned(),
            other => format!("{} usage", other.display_name()),
        };
        format!(
            "{subject} is at {:.0}% (threshold {:.0}%).",
            self.used_percent, self.threshold
        )
    }

    /// The request id / toast tag: `<provider>-usage-threshold-<Int(threshold)>`.
    pub fn id(&self) -> String {
        format!(
            "{}-usage-threshold-{}",
            self.provider.as_str(),
            self.threshold as i64
        )
    }
}

/// Delivers a [`ThresholdAlert`] to the user (OS toast in production).
pub trait Notifier: Send + Sync {
    /// Shows `alert` immediately. Failures are the implementation's to log.
    fn deliver(&self, alert: &ThresholdAlert);
}

/// A [`Notifier`] that records alerts, for tests and headless runs.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    alerts: Mutex<Vec<ThresholdAlert>>,
}

impl RecordingNotifier {
    /// Alerts delivered so far, oldest first.
    pub fn alerts(&self) -> Vec<ThresholdAlert> {
        self.alerts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Notifier for RecordingNotifier {
    fn deliver(&self, alert: &ThresholdAlert) {
        self.alerts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(alert.clone());
    }
}

/// Persistent `Double` records keyed by the `thresholdNotified.*` names.
pub trait NotifiedStore: Send + Sync {
    /// Reads a number, `None` when absent or not numeric.
    fn get_f64(&self, key: &str) -> Option<f64>;
    /// Writes a number.
    fn set_f64(&self, key: &str, value: f64);
    /// Removes a key.
    fn remove(&self, key: &str);
}

impl NotifiedStore for SettingsFile {
    fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).as_ref().and_then(Value::as_f64)
    }

    fn set_f64(&self, key: &str, value: f64) {
        self.set(key, Value::from(value));
    }

    fn remove(&self, key: &str) {
        Self::remove(self, key);
    }
}

/// The pure crossing rule: fire at or above `threshold` unless a notification at
/// or above that threshold is already on record.
///
/// # Examples
///
/// ```
/// use tokenmon_core::alerts::should_notify;
///
/// assert!(should_notify(80.0, 80.0, None));
/// assert!(!should_notify(79.9, 80.0, None));
/// assert!(!should_notify(90.0, 80.0, Some(80.0)));
/// assert!(should_notify(90.0, 85.0, Some(80.0)));
/// ```
pub fn should_notify(used_percent: f64, threshold: f64, last_notified: Option<f64>) -> bool {
    used_percent >= threshold && !last_notified.is_some_and(|last| last >= threshold)
}

/// The pure re-arm rule: usage strictly more than five points below the record.
///
/// # Examples
///
/// ```
/// use tokenmon_core::alerts::should_rearm;
///
/// assert!(!should_rearm(74.0, Some(75.0)));
/// assert!(!should_rearm(70.0, Some(75.0)));
/// assert!(should_rearm(69.0, Some(75.0)));
/// ```
pub fn should_rearm(used_percent: f64, last_notified: Option<f64>) -> bool {
    last_notified.is_some_and(|last| used_percent < last - REARM_DROP_POINTS)
}

/// Fires [`ThresholdAlert`]s when usage crosses the user's threshold.
///
/// State lives in a [`NotifiedStore`], scoped per provider and per account, so it
/// survives a relaunch.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use tokenmon_core::ProviderId;
/// use tokenmon_core::alerts::{RecordingNotifier, ThresholdNotifier};
/// use tokenmon_core::settings::{AppSettings, SettingsFile};
///
/// let file = Arc::new(SettingsFile::in_memory());
/// let settings = AppSettings::new(Arc::clone(&file));
/// settings.set_threshold_enabled(true);
/// let sink = Arc::new(RecordingNotifier::default());
/// let notifier = ThresholdNotifier::new(file, sink.clone());
/// notifier.evaluate(ProviderId::Grok, 85.0, &settings.snapshot(), None, None);
/// notifier.evaluate(ProviderId::Grok, 86.0, &settings.snapshot(), None, None);
/// assert_eq!(sink.alerts().len(), 1);
/// ```
#[derive(Clone)]
pub struct ThresholdNotifier {
    store: Arc<dyn NotifiedStore>,
    sink: Arc<dyn Notifier>,
}

impl std::fmt::Debug for ThresholdNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThresholdNotifier").finish_non_exhaustive()
    }
}

/// The record scope: bare account for Grok (legacy key shape), else
/// `<provider>.<account>`; a missing account is `default`.
pub fn scope_key(provider: ProviderId, account: Option<&str>) -> String {
    let account = account.unwrap_or("default");
    if provider == ProviderId::Grok {
        account.to_owned()
    } else {
        format!("{}.{account}", provider.as_str())
    }
}

impl ThresholdNotifier {
    /// Creates a notifier that records in `store` and delivers through `sink`.
    pub fn new(store: Arc<dyn NotifiedStore>, sink: Arc<dyn Notifier>) -> Self {
        Self { store, sink }
    }

    /// Evaluates one usage reading (spec 01 section 5).
    ///
    /// Does nothing unless the alert watches `provider`. `account` scopes the
    /// record; `resets_at` is the end of the current billing period.
    pub fn evaluate(
        &self,
        provider: ProviderId,
        used_percent: f64,
        settings: &Settings,
        account: Option<&str>,
        resets_at: Option<DateTime<Utc>>,
    ) {
        if !settings.alert_watches(provider) || used_percent.is_nan() {
            return;
        }
        let threshold = settings.threshold_percent;
        let scope = scope_key(provider, account);
        let key = format!("{NOTIFIED_PREFIX}{scope}");
        let period_key = format!("{PERIOD_PREFIX}{scope}");
        let resets_unix = resets_at.map(|t| t.timestamp() as f64);

        let last = self.refresh_period(&key, &period_key, resets_unix);

        if used_percent < threshold {
            if should_rearm(used_percent, last) {
                self.store.remove(&key);
                self.store.remove(&period_key);
            }
            return;
        }
        if !should_notify(used_percent, threshold, last) {
            return;
        }
        self.store.set_f64(&key, threshold);
        if let Some(unix) = resets_unix {
            self.store.set_f64(&period_key, unix);
        }
        self.sink.deliver(&ThresholdAlert {
            provider,
            used_percent,
            threshold,
        });
    }

    /// Reads the last record, clearing it when `resets_unix` starts a later
    /// period and adopting the first period seen otherwise.
    fn refresh_period(&self, key: &str, period_key: &str, resets_unix: Option<f64>) -> Option<f64> {
        let last = self.store.get_f64(key)?;
        let Some(resets) = resets_unix else {
            return Some(last);
        };
        match self.store.get_f64(period_key) {
            Some(recorded) if resets > recorded + PERIOD_SLACK_SECONDS => {
                self.store.remove(key);
                self.store.remove(period_key);
                None
            }
            None => {
                self.store.set_f64(period_key, resets);
                Some(last)
            }
            Some(_) => Some(last),
        }
    }
}
