//! Stable-toolchain regressions for the settings decoder: every corrupt or
//! hostile `settings.json` must decode to valid, clamped settings.

use tokenmon_core::ProviderId;
use tokenmon_core::settings::{Settings, SettingsKey, SettingsMap, decode_settings};

fn assert_valid(s: &Settings) {
    assert!((15..=300).contains(&s.active_poll_seconds));
    assert!((60..=3600).contains(&s.idle_poll_seconds));
    assert!((50.0..=99.0).contains(&s.threshold_percent));
    assert!(!s.enabled_provider_ids.is_empty());
    assert_eq!(
        ProviderId::normalized_order(&s.provider_order),
        s.provider_order
    );
    let encoded: SettingsMap = SettingsKey::ALL
        .iter()
        .map(|k| (k.as_str().to_owned(), k.encode(s)))
        .collect();
    assert_eq!(&Settings::from_map(&encoded), s, "decode is idempotent");
}

/// Inputs that are not a JSON object, or not JSON at all, give defaults.
#[test]
fn test_non_object_and_garbage_files_decode_to_defaults() {
    let inputs: [&[u8]; 9] = [
        b"",
        b"null",
        b"[]",
        b"42",
        b"\"str\"",
        b"{",
        b"\xff\xfe\x00",
        b"{\"a\":",
        &[b'['; 4096],
    ];
    for input in inputs {
        assert_eq!(decode_settings(input), Settings::default(), "{input:?}");
    }
}

/// Wrong-typed values fall back per key instead of failing the whole file.
#[test]
fn test_wrong_types_fall_back_per_key() {
    let s = decode_settings(
        br#"{"activePollSeconds":"x","idlePollSeconds":[1],"thresholdPercent":{},
             "thresholdEnabled":"yes","providerOrder":"grok","enabledProviderIDs":7,
             "collapsedPanelSections":{"a":1},"selectedProvider":null,
             "menuBarGraphProviders":5,"visibleProductIDs":false}"#,
    );
    assert_eq!(s, Settings::default());
}

/// Extreme numbers clamp; out-of-range JSON numbers fail only the syntax.
#[test]
fn test_extreme_numbers_clamp() {
    let s = decode_settings(
        br#"{"activePollSeconds":-9223372036854775808,"idlePollSeconds":18446744073709551615,"thresholdPercent":1.7e308}"#,
    );
    assert_valid(&s);
    assert_eq!((s.active_poll_seconds, s.idle_poll_seconds), (15, 3600));
    assert!((s.threshold_percent - 99.0).abs() < f64::EPSILON);
    assert_valid(&decode_settings(br#"{"thresholdPercent":1e999}"#));
    assert_valid(&decode_settings(
        br#"{"activePollSeconds":1e30,"idlePollSeconds":-0.5}"#,
    ));
}

/// Duplicates, unknown entries and non-strings in lists are normalized.
#[test]
fn test_list_normalization() {
    let s = decode_settings(
        br#"{"providerOrder":["claude","claude","overview","x",1,null],
             "alertProviders":["overview","grok","grok"],
             "enabledProviderIDs":["overview"],
             "menuBarGraphProviders":["openrouter","grok","chatgpt"]}"#,
    );
    assert_valid(&s);
    assert_eq!(s.alert_providers.len(), 1);
    assert_eq!(s.enabled_provider_ids.len(), 7);
    assert_eq!(s.menu_bar_graph_providers.len(), 1);
}
