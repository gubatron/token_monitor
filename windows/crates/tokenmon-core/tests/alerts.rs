//! Port of `ThresholdNotifierTests` (spec 01 sections 5 and 7).

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::ProviderId::{self, *};
use tokenmon_core::alerts::{
    RecordingNotifier, ThresholdAlert, ThresholdNotifier, should_notify, should_rearm,
};
use tokenmon_core::settings::{AppSettings, Settings, SettingsFile};

struct Rig {
    file: Arc<SettingsFile>,
    sink: Arc<RecordingNotifier>,
    notifier: ThresholdNotifier,
    settings: Settings,
}

fn rig(threshold: f64) -> Rig {
    let file = Arc::new(SettingsFile::in_memory());
    let app = AppSettings::new(Arc::clone(&file));
    app.set_threshold_enabled(true);
    app.set_threshold_percent(threshold);
    let sink = Arc::new(RecordingNotifier::default());
    let notifier = ThresholdNotifier::new(file.clone(), sink.clone());
    Rig {
        file,
        sink,
        notifier,
        settings: app.snapshot(),
    }
}

impl Rig {
    fn eval(&self, p: ProviderId, used: f64) {
        self.notifier.evaluate(p, used, &self.settings, None, None);
    }
}

/// The pure crossing and re-arm rules.
#[test]
fn test_should_notify_rules() {
    assert!(should_notify(81.0, 80.0, None));
    assert!(!should_notify(79.0, 80.0, None));
    assert!(should_notify(80.0, 80.0, None), "exact boundary counts");
    assert!(!should_notify(90.0, 80.0, Some(80.0)));
    assert!(
        should_notify(90.0, 85.0, Some(80.0)),
        "raised threshold refires"
    );
    assert!(should_rearm(69.0, Some(75.0)));
    assert!(!should_rearm(74.0, Some(75.0)));
    assert!(!should_rearm(70.0, Some(75.0)), "strictly below");
    assert!(!should_rearm(10.0, None));
}

/// Fires once and not again while usage stays above the threshold.
#[test]
fn test_evaluate_fires_once_while_above() {
    let r = rig(80.0);
    r.eval(Grok, 85.0);
    r.eval(Grok, 90.0);
    r.eval(Grok, 85.0);
    assert_eq!(r.sink.alerts().len(), 1);
    assert_eq!(r.sink.alerts()[0].used_percent, 85.0);
}

/// Below the threshold nothing fires.
#[test]
fn test_does_not_fire_below_threshold() {
    let r = rig(80.0);
    r.eval(Grok, 79.9);
    assert_eq!(r.sink.alerts().len(), 0);
}

/// A new notifier on the same store stays quiet (state survives relaunch).
#[test]
fn test_evaluate_persists_across_instances() {
    let r = rig(80.0);
    r.eval(Grok, 85.0);
    let again = ThresholdNotifier::new(r.file.clone(), r.sink.clone());
    again.evaluate(Grok, 86.0, &r.settings, None, None);
    assert_eq!(r.sink.alerts().len(), 1);
}

/// Different accounts alert independently.
#[test]
fn test_evaluate_is_account_scoped() {
    let r = rig(80.0);
    r.notifier
        .evaluate(Grok, 85.0, &r.settings, Some("a@b.com"), None);
    r.notifier
        .evaluate(Grok, 85.0, &r.settings, Some("c@d.com"), None);
    r.notifier
        .evaluate(Grok, 85.0, &r.settings, Some("a@b.com"), None);
    assert_eq!(r.sink.alerts().len(), 2);
}

/// Record keys are `thresholdNotified.<account>` for Grok and
/// `thresholdNotified.<provider>.<account>` otherwise.
#[test]
fn test_evaluate_is_provider_scoped() {
    let r = rig(80.0);
    r.notifier
        .evaluate(Grok, 85.0, &r.settings, Some("a@b.com"), None);
    r.notifier
        .evaluate(Claude, 85.0, &r.settings, Some("a@b.com"), None);
    r.notifier.evaluate(Claude, 85.0, &r.settings, None, None);
    let map = r.file.snapshot();
    assert!(map.contains_key("thresholdNotified.a@b.com"));
    assert!(map.contains_key("thresholdNotified.claude.a@b.com"));
    assert!(map.contains_key("thresholdNotified.claude.default"));
    assert_eq!(r.sink.alerts().len(), 3);
}

/// Exact bodies, title and id.
#[test]
fn test_notification_text() {
    let grok = ThresholdAlert {
        provider: Grok,
        used_percent: 85.4,
        threshold: 80.0,
    };
    assert_eq!(
        grok.body(),
        "Weekly SuperGrok usage is at 85% (threshold 80%)."
    );
    assert_eq!(grok.title(), "TokenMon Alert");
    assert_eq!(grok.id(), "grok-usage-threshold-80");
    let claude = ThresholdAlert {
        provider: Claude,
        used_percent: 91.0,
        threshold: 90.0,
    };
    assert_eq!(claude.body(), "Claude usage is at 91% (threshold 90%).");
}

/// 81 fires, 74 stays quiet, 69 clears the record, 76 fires again (threshold 75).
#[test]
fn test_evaluate_rearms_after_five_point_drop() {
    let r = rig(75.0);
    r.eval(Grok, 81.0);
    r.eval(Grok, 74.0);
    r.eval(Grok, 81.0);
    assert_eq!(r.sink.alerts().len(), 1);
    r.eval(Grok, 69.0);
    r.eval(Grok, 76.0);
    assert_eq!(r.sink.alerts().len(), 2);
}

/// A disabled alert or an unwatched provider does nothing.
#[test]
fn test_evaluate_is_noop_when_disabled_or_unwatched() {
    let mut r = rig(80.0);
    r.settings.threshold_enabled = false;
    r.eval(Grok, 99.0);
    r.settings.threshold_enabled = true;
    r.settings.alert_providers.remove(&Grok);
    r.eval(Grok, 99.0);
    assert_eq!(r.sink.alerts().len(), 0);
    assert!(
        r.file
            .snapshot()
            .keys()
            .all(|k| !k.starts_with("thresholdNotified"))
    );
}

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("valid")
}

/// +300 s is the same period; +7 days is a new one; then quiet again.
#[test]
fn test_evaluate_rearms_for_new_billing_period() {
    let r = rig(80.0);
    let n = &r.notifier;
    let base = 1_800_000_000;
    n.evaluate(Grok, 85.0, &r.settings, None, Some(at(base)));
    n.evaluate(Grok, 85.0, &r.settings, None, Some(at(base + 300)));
    assert_eq!(r.sink.alerts().len(), 1);
    n.evaluate(Grok, 85.0, &r.settings, None, Some(at(base + 7 * 86_400)));
    assert_eq!(r.sink.alerts().len(), 2);
    n.evaluate(Grok, 85.0, &r.settings, None, Some(at(base + 7 * 86_400)));
    assert_eq!(r.sink.alerts().len(), 2);
}

/// A record without a period adopts the first reset time it sees.
#[test]
fn test_period_is_adopted_when_missing() {
    let r = rig(80.0);
    r.eval(Grok, 85.0);
    r.notifier
        .evaluate(Grok, 85.0, &r.settings, None, Some(at(1_800_000_000)));
    assert!(
        r.file
            .snapshot()
            .contains_key("thresholdNotifiedResetsAt.default")
    );
    assert_eq!(r.sink.alerts().len(), 1);
}

/// Corrupt non-numeric records behave as absent instead of panicking.
#[test]
fn test_non_numeric_records_are_ignored() {
    let r = rig(80.0);
    r.file
        .set("thresholdNotified.default", serde_json::json!("oops"));
    r.eval(Grok, 85.0);
    assert_eq!(r.sink.alerts().len(), 1);
}

/// A sink whose delivery fails until `ok` is set, counting attempts.
struct FlakySink {
    ok: std::sync::atomic::AtomicBool,
    attempts: std::sync::atomic::AtomicUsize,
}

impl tokenmon_core::alerts::Notifier for FlakySink {
    fn deliver(&self, _alert: &ThresholdAlert) {
        self.try_deliver(_alert);
    }
    fn try_deliver(&self, _alert: &ThresholdAlert) -> bool {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.ok.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Regression (#73): a crossing whose toast failed is not recorded as
/// announced, so the next poll retries; after a success it fires only once.
#[test]
fn test_failed_delivery_is_retried_then_recorded_once_delivered() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let file = Arc::new(SettingsFile::in_memory());
    let app = AppSettings::new(Arc::clone(&file));
    app.set_threshold_enabled(true);
    app.set_threshold_percent(80.0);
    let sink = Arc::new(FlakySink {
        ok: AtomicBool::new(false),
        attempts: AtomicUsize::new(0),
    });
    let notifier = ThresholdNotifier::new(file, sink.clone());
    let settings = app.snapshot();
    notifier.evaluate(Grok, 85.0, &settings, None, None);
    notifier.evaluate(Grok, 86.0, &settings, None, None);
    assert_eq!(sink.attempts.load(Ordering::SeqCst), 2);
    sink.ok.store(true, Ordering::SeqCst);
    notifier.evaluate(Grok, 87.0, &settings, None, None);
    notifier.evaluate(Grok, 88.0, &settings, None, None);
    assert_eq!(sink.attempts.load(Ordering::SeqCst), 3);
}
