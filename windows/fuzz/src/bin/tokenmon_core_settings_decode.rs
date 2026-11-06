//! Decodes arbitrary bytes as `settings.json`; corrupt input must yield valid,
//! clamped settings that round-trip through their stored form.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::ProviderId;
use tokenmon_core::settings::{Settings, SettingsKey, SettingsMap, decode_settings};

fuzz_target!(|data: &[u8]| {
    let s = decode_settings(data);
    assert!((15..=300).contains(&s.active_poll_seconds));
    assert!((60..=3600).contains(&s.idle_poll_seconds));
    assert!((50.0..=99.0).contains(&s.threshold_percent));
    assert!(!s.enabled_provider_ids.is_empty());
    assert!(
        s.enabled_provider_ids
            .iter()
            .all(|p| ProviderId::USAGE_PROVIDERS.contains(p))
    );
    assert!(
        s.alert_providers
            .iter()
            .all(|p| ProviderId::USAGE_PROVIDERS.contains(p))
    );
    assert!(
        s.menu_bar_graph_providers
            .iter()
            .all(|p| p.supports_menu_bar_graph())
    );
    assert_eq!(
        ProviderId::normalized_order(&s.provider_order),
        s.provider_order
    );

    let encoded: SettingsMap = SettingsKey::ALL
        .iter()
        .map(|k| (k.as_str().to_owned(), k.encode(&s)))
        .collect();
    assert_eq!(Settings::from_map(&encoded), s);
});
