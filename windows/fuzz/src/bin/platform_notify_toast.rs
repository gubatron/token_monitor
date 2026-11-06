//! Invariants: parsing activation arguments never panics and a parsed provider
//! round-trips through `provider=<id>`; `xml_escape` leaves no raw XML special
//! characters; `ToastContent::from_alert` bounds the tag and its launch
//! arguments always parse back to the alert's provider.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::ProviderId;
use tokenmon_core::alerts::ThresholdAlert;
use tokenmon_platform::notify::toast::{MAX_TAG_LEN, xml_escape};
use tokenmon_platform::notify::{ToastContent, parse_activation_arguments};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Some(provider) = parse_activation_arguments(&text) {
        let again = format!("provider={}", provider.as_str());
        assert_eq!(parse_activation_arguments(&again), Some(provider));
    }
    let escaped = xml_escape(&text);
    assert!(!escaped.contains(['<', '>', '"', '\'']));
    assert!(escaped.len() >= text.len());

    let (used, threshold) = match data {
        [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, ..] => (
            f64::from_le_bytes([*a, *b, *c, *d, *e, *f, *g, *h]),
            f64::from_le_bytes([*i, *j, *k, *l, *m, *n, *o, *p]),
        ),
        _ => (50.0, 80.0),
    };
    for provider in ProviderId::ALL_CASES {
        let alert = ThresholdAlert {
            provider,
            used_percent: used,
            threshold,
        };
        let content = ToastContent::from_alert(&alert);
        assert!(content.tag.chars().count() <= MAX_TAG_LEN);
        assert_eq!(
            parse_activation_arguments(&content.launch_arguments),
            Some(provider)
        );
        let _ = content.to_xml();
    }
});
