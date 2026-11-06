//! Hermetic tests for toast composition, activation and AUMID registration.

use std::sync::{Arc, Mutex};

use tokenmon_core::ProviderId;
use tokenmon_core::alerts::{Notifier, ThresholdAlert};
use tokenmon_platform::notify::toast::xml_escape;
use tokenmon_platform::notify::{
    ActivationHandler, AumidRegistration, AumidStatus, FakeToastBackend, ToastContent,
    ToastNotifier, parse_activation_arguments,
};
use tokenmon_platform::shell::{MemoryRegistry, Registry};

type Clicks = Arc<Mutex<Vec<ProviderId>>>;

fn alert(provider: ProviderId, used: f64, threshold: f64) -> ThresholdAlert {
    ThresholdAlert {
        provider,
        used_percent: used,
        threshold,
    }
}

fn notifier() -> (
    Arc<FakeToastBackend>,
    Clicks,
    ToastNotifier<Arc<FakeToastBackend>>,
) {
    let backend = Arc::new(FakeToastBackend::new());
    let clicks: Clicks = Arc::new(Mutex::new(Vec::new()));
    let sink = clicks.clone();
    let handler: ActivationHandler = Arc::new(move |p| sink.lock().unwrap().push(p));
    (
        backend.clone(),
        clicks,
        ToastNotifier::new(backend, handler),
    )
}

/// Title, body and tag match spec 01 section 5 exactly.
#[test]
fn test_toast_text_matches_spec() {
    let t = ToastContent::from_alert(&alert(ProviderId::Grok, 85.4, 80.0));
    assert_eq!(t.title, "TokenMon Alert");
    assert_eq!(t.body, "Weekly SuperGrok usage is at 85% (threshold 80%).");
    assert_eq!(t.tag, "grok-usage-threshold-80");
    let c = ToastContent::from_alert(&alert(ProviderId::Claude, 91.0, 90.0));
    assert!(c.body.starts_with("Claude usage is at 91%"), "{}", c.body);
}

/// The XML is ToastGeneric with two text nodes and the launch arguments.
#[test]
fn test_toast_xml_shape() {
    let xml = ToastContent::from_alert(&alert(ProviderId::Claude, 85.0, 80.0)).to_xml();
    assert!(xml.starts_with("<toast launch=\"provider=claude\">"));
    assert!(xml.contains("template=\"ToastGeneric\""));
    assert!(xml.contains("<text>TokenMon Alert</text>"));
    assert_eq!(xml.matches("<text>").count(), 2);
}

/// Special characters cannot break out of the XML.
#[test]
fn test_xml_escape() {
    assert_eq!(xml_escape("a&b<c>\"d'"), "a&amp;b&lt;c&gt;&quot;d&apos;");
    assert_eq!(xml_escape("plain"), "plain");
}

/// Activation arguments roundtrip for every provider and reject junk.
#[test]
fn test_activation_arguments_roundtrip_and_rejection() {
    for p in ProviderId::ALL_CASES {
        let t = ToastContent::from_alert(&alert(p, 90.0, 80.0));
        assert_eq!(parse_activation_arguments(&t.launch_arguments), Some(p));
    }
    for bad in [
        "",
        "provider=",
        "provider=nope",
        "x=claude",
        "provider=claude&x=1",
    ] {
        assert_eq!(parse_activation_arguments(bad), None, "{bad}");
    }
}

/// Delivering an alert shows exactly one toast with the composed content.
#[test]
fn test_deliver_shows_toast() {
    let (backend, _, n) = notifier();
    n.deliver(&alert(ProviderId::Grok, 85.0, 80.0));
    let shown = backend.shown();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].tag, "grok-usage-threshold-80");
}

/// Clicking a toast opens the panel on the alert's provider.
#[test]
fn test_click_invokes_callback_with_provider() {
    let (backend, clicks, n) = notifier();
    n.deliver(&alert(ProviderId::Claude, 85.0, 80.0));
    n.deliver(&alert(ProviderId::Grok, 85.0, 80.0));
    backend.click(1);
    backend.click(0);
    backend.click(9);
    assert_eq!(
        *clicks.lock().unwrap(),
        vec![ProviderId::Grok, ProviderId::Claude]
    );
}

/// A backend failure is swallowed (logged), never panics.
#[test]
fn test_backend_failure_is_not_fatal() {
    let (backend, _, n) = notifier();
    backend.set_fail(true);
    n.deliver(&alert(ProviderId::Grok, 85.0, 80.0));
    assert!(backend.shown().is_empty());
}

/// First run registers the AUMID; the second run writes nothing.
#[test]
fn test_aumid_registration_is_idempotent() {
    let reg = Arc::new(MemoryRegistry::new());
    let r = AumidRegistration::new(reg.clone(), Some(r"C:\TokenMon\icon.ico".into()));
    assert_eq!(r.ensure().unwrap(), AumidStatus::Registered);
    let key = AumidRegistration::key();
    assert_eq!(key, r"Software\Classes\AppUserModelId\TokenMon");
    assert_eq!(
        reg.get_string(&key, "DisplayName").unwrap().as_deref(),
        Some("TokenMon")
    );
    assert_eq!(
        reg.get_string(&key, "IconUri").unwrap().as_deref(),
        Some(r"C:\TokenMon\icon.ico")
    );
    reg.set_fail_writes(true);
    assert_eq!(r.ensure().unwrap(), AumidStatus::AlreadyRegistered);
}

/// A moved install (new icon path) re-registers; removal is idempotent.
#[test]
fn test_aumid_updates_stale_icon_and_removes() {
    let reg = Arc::new(MemoryRegistry::new());
    AumidRegistration::new(reg.clone(), Some("a.ico".into()))
        .ensure()
        .unwrap();
    let r = AumidRegistration::new(reg.clone(), Some("b.ico".into()));
    assert_eq!(r.ensure().unwrap(), AumidStatus::Registered);
    r.remove().unwrap();
    r.remove().unwrap();
    assert!(!reg.key_exists(&AumidRegistration::key()).unwrap());
}

/// Registry write failures surface as typed errors.
#[test]
fn test_aumid_write_failure_is_reported() {
    let reg = Arc::new(MemoryRegistry::new());
    reg.set_fail_writes(true);
    assert!(AumidRegistration::new(reg, None).ensure().is_err());
}
