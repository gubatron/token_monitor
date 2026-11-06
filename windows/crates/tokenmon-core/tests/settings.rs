//! Port of `Settings/AppSettingsTests` (spec 01 sections 2 and 7).

use std::collections::BTreeSet;
use std::sync::Arc;

use tokenmon_core::ProviderId::{self, *};
use tokenmon_core::settings::{
    AppSettings, GrokProduct, PanelSection, Settings, SettingsFile, decode_settings,
};

fn fresh() -> (tempfile::TempDir, AppSettings) {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = AppSettings::open(dir.path().join("settings.json"));
    (dir, s)
}

fn reopen(dir: &tempfile::TempDir) -> AppSettings {
    AppSettings::open(dir.path().join("settings.json"))
}

fn set_of(ps: &[ProviderId]) -> BTreeSet<ProviderId> {
    ps.iter().copied().collect()
}

/// Poll intervals clamp on set and on load.
#[test]
fn test_poll_intervals_are_clamped_on_set_and_load() {
    let (dir, s) = fresh();
    s.set_active_poll_seconds(1);
    assert_eq!(s.snapshot().active_poll_seconds, 15);
    s.set_active_poll_seconds(9999);
    assert_eq!(s.snapshot().active_poll_seconds, 300);
    s.set_idle_poll_seconds(5);
    assert_eq!(s.snapshot().idle_poll_seconds, 60);
    s.set_idle_poll_seconds(99_999);
    assert_eq!(s.snapshot().idle_poll_seconds, 3600);
    drop(s);
    let loaded = decode_settings(br#"{"activePollSeconds": 0, "idlePollSeconds": 1e9}"#);
    assert_eq!(
        (loaded.active_poll_seconds, loaded.idle_poll_seconds),
        (15, 3600)
    );
    assert_eq!(reopen(&dir).snapshot().idle_poll_seconds, 3600);
}

/// Defaults match the spec table.
#[test]
fn test_defaults() {
    let s = Settings::default();
    assert!(s.show_categories_in_menu_bar && s.checks_for_updates);
    assert!(!s.show_selected_provider_in_menu_bar && !s.threshold_enabled);
    assert_eq!((s.active_poll_seconds, s.idle_poll_seconds), (60, 300));
    assert!((s.threshold_percent - 80.0).abs() < f64::EPSILON);
    assert_eq!(s.menu_bar_graph_providers, set_of(&[Grok]));
    assert_eq!(s.selected_provider, Grok);
    assert_eq!(s.provider_order, ProviderId::USAGE_PROVIDERS);
    assert_eq!(s.enabled_provider_ids.len(), 7);
    assert_eq!(s.alert_providers.len(), 7);
    assert_eq!(s.visible_product_ids.len(), 6);
    assert!(s.collapsed_panel_sections.is_empty());
    assert_eq!(decode_settings(b""), s);
}

/// Values survive a reopen.
#[test]
fn test_values_persist_across_instances() {
    let (dir, s) = fresh();
    s.set_active_poll_seconds(30);
    s.set_menu_bar_graph(Claude, true);
    s.set_show_selected_provider_in_menu_bar(true);
    s.set_threshold_enabled(true);
    s.set_checks_for_updates(false);
    let loaded = reopen(&dir).snapshot();
    assert_eq!(loaded.active_poll_seconds, 30);
    assert_eq!(loaded.menu_bar_graph_providers, set_of(&[Grok, Claude]));
    assert!(loaded.show_selected_provider_in_menu_bar && loaded.threshold_enabled);
    assert!(!loaded.checks_for_updates);
}

/// Panel sections default open, persist, and ignore unknown stored values.
#[test]
fn test_panel_sections_default_open_persist_and_ignore_unknown() {
    let (dir, s) = fresh();
    assert!(s.snapshot().is_expanded(PanelSection::ClaudeModels));
    s.set_expanded(false, PanelSection::ClaudeModels);
    assert!(
        !reopen(&dir)
            .snapshot()
            .is_expanded(PanelSection::ClaudeModels)
    );
    let loaded = decode_settings(br#"{"collapsedPanelSections":["openCodeModels","bogus",3]}"#);
    assert_eq!(
        loaded.collapsed_panel_sections,
        BTreeSet::from([PanelSection::OpenCodeModels])
    );
}

/// Threshold clamps on load (250 -> 99, -5 -> 50) and on set (10 -> 50, persisted).
#[test]
fn test_threshold_percent_is_clamped_on_load_and_set() {
    assert!(
        (decode_settings(br#"{"thresholdPercent":250}"#).threshold_percent - 99.0).abs() < 1e-9
    );
    assert!((decode_settings(br#"{"thresholdPercent":-5}"#).threshold_percent - 50.0).abs() < 1e-9);
    let (dir, s) = fresh();
    s.set_threshold_percent(10.0);
    assert!((reopen(&dir).snapshot().threshold_percent - 50.0).abs() < 1e-9);
    let text = std::fs::read_to_string(dir.path().join("settings.json")).expect("file");
    assert!(text.contains("\"thresholdPercent\": 50.0"));
}

fn polling(s: &Settings) -> Vec<ProviderId> {
    ProviderId::USAGE_PROVIDERS
        .into_iter()
        .filter(|p| s.needs_polling(*p))
        .collect()
}

/// `needsPolling` formula: tab, Overview, pinned bars, alert, enablement.
#[test]
fn test_needs_polling_formula() {
    let s = Settings::default();
    assert_eq!(polling(&s), vec![Grok]);
    let s = s.with_selected_provider(Claude);
    assert_eq!(polling(&s), vec![Grok, Claude]);
    let off = s.clone().with_menu_bar_graph(Grok, false);
    assert_eq!(polling(&off), vec![Claude]);
    let overview = Settings::default().with_selected_provider(Overview);
    assert_eq!(polling(&overview).len(), 7);
    assert!(!overview.needs_polling(Overview));
    let on = Settings::default().with_menu_bar_graph(Grok, false);
    assert!(
        on.clone()
            .with_menu_bar_graph(Grokbot, true)
            .needs_polling(Grokbot)
    );
    assert!(
        on.clone()
            .with_menu_bar_graph(Claude, true)
            .needs_polling(Claude)
    );
    assert!(on.with_selected_provider(Cursor).needs_polling(Cursor));
}

/// An enabled alert keeps its providers polling, restricted by `alertProviders`.
#[test]
fn test_threshold_alert_keeps_alert_providers_polling() {
    let s = Settings::default().with_menu_bar_graph(Grok, false);
    let s = Settings {
        threshold_enabled: true,
        ..s
    };
    assert_eq!(polling(&s).len(), 7);
    let s = s
        .with_selected_provider(Claude)
        .with_alert_providers(&set_of(&[Claude, Overview]));
    assert_eq!(polling(&s), vec![Claude]);
}

/// A disabled provider never polls, even with its bar pinned.
#[test]
fn test_disabled_provider_does_not_poll() {
    let s = Settings::default()
        .with_menu_bar_graph(Grokbot, true)
        .with_enabled_providers(&set_of(&[Grok, Claude]))
        .expect("valid");
    assert!(!s.needs_polling(Grokbot));
    assert!(s.needs_polling(Grok));
}

/// `pollInterval` combines `needsPolling` with the open/closed interval.
#[test]
fn test_poll_interval_selects_active_or_idle() {
    use std::time::Duration;
    let s = Settings::default();
    assert_eq!(s.poll_interval(Grok, true), Some(Duration::from_secs(60)));
    assert_eq!(s.poll_interval(Grok, false), Some(Duration::from_secs(300)));
    assert_eq!(s.poll_interval(Claude, true), None);
}

/// Provider list rules: defaults, order, move, enabled validation.
#[test]
fn test_provider_lists() {
    let s = Settings::default();
    assert_eq!(
        s.visible_usage_providers(),
        ProviderId::USAGE_PROVIDERS.to_vec()
    );
    let moved = s.clone().move_provider(Claude, Grok);
    assert_eq!(&moved.provider_order[..3], &[Claude, Grok, Cursor]);
    let back = moved.move_provider(Claude, Grokbot);
    assert_eq!(back.provider_order[6], Claude);
    let norm = s.with_provider_order(&[Overview, Claude, Claude, Cursor]);
    assert_eq!(&norm.provider_order[..2], &[Claude, Cursor]);
    assert_eq!(norm.provider_order.len(), 7);
}

/// Order persists in its normalized form and appends newly added providers.
#[test]
fn test_provider_order_persists_normalized_value() {
    let (dir, s) = fresh();
    s.set_provider_order(&[Overview, Claude, Claude]);
    let text = std::fs::read_to_string(dir.path().join("settings.json")).expect("file");
    assert!(text.contains("\"claude\",\n    \"grok\""));
    assert_eq!(reopen(&dir).snapshot().provider_order[0], Claude);
    let old = decode_settings(br#"{"providerOrder":["claude","zzz"]}"#);
    assert_eq!(old.provider_order[0], Claude);
    assert_eq!(old.provider_order.len(), 7);
}

/// Empty or non-usage enabled sets are rejected; stored junk is ignored on load.
#[test]
fn test_enabled_providers_validation() {
    let (dir, s) = fresh();
    assert!(s.set_enabled_providers(&BTreeSet::new()).is_err());
    assert!(s.set_enabled_providers(&set_of(&[Grok, Overview])).is_err());
    assert_eq!(s.snapshot().enabled_provider_ids.len(), 7);
    s.set_enabled_providers(&set_of(&[Grok, Cursor]))
        .expect("valid");
    assert_eq!(
        reopen(&dir).snapshot().enabled_provider_ids,
        set_of(&[Grok, Cursor])
    );
    let junk = decode_settings(br#"{"enabledProviderIDs":["overview","nope"]}"#);
    assert_eq!(junk.enabled_provider_ids.len(), 7);
    let subset = decode_settings(br#"{"enabledProviderIDs":["claude","overview"]}"#);
    assert_eq!(subset.enabled_provider_ids, set_of(&[Claude]));
}

/// Disabling the selected provider falls back to Overview (persisted); others keep it.
#[test]
fn test_disabling_selected_provider_falls_back_to_overview() {
    let (dir, s) = fresh();
    s.set_enabled_providers(&set_of(&[Claude, Cursor]))
        .expect("valid");
    assert_eq!(s.snapshot().selected_provider, Overview);
    assert_eq!(reopen(&dir).snapshot().selected_provider, Overview);
    let (_d2, s2) = fresh();
    s2.set_selected_provider(Cursor);
    s2.set_enabled_providers(&set_of(&[Cursor, Claude]))
        .expect("valid");
    assert_eq!(s2.snapshot().selected_provider, Cursor);
}

/// Grok product rules: retired ids fall back to all, empty is preserved, unknown dropped.
#[test]
fn test_visible_products_load_rules() {
    let all = decode_settings(br#"{"visibleProductIDs":["legacy"]}"#);
    assert_eq!(all.visible_product_ids.len(), 6);
    let empty = decode_settings(br#"{"visibleProductIDs":[]}"#);
    assert!(empty.visible_product_ids.is_empty());
    let some = decode_settings(br#"{"visibleProductIDs":["Build","chat","zzz"]}"#);
    assert_eq!(
        some.visible_product_ids,
        BTreeSet::from([GrokProduct::Build, GrokProduct::Chat])
    );
    let alerts = decode_settings(br#"{"alertProviders":[]}"#);
    assert!(alerts.alert_providers.is_empty());
}

/// Unknown selected provider falls back to Grok; Overview is valid.
#[test]
fn test_selected_provider_decoding() {
    assert_eq!(
        decode_settings(br#"{"selectedProvider":"overview"}"#).selected_provider,
        Overview
    );
    assert_eq!(
        decode_settings(br#"{"selectedProvider":"bogus"}"#).selected_provider,
        Grok
    );
    assert_eq!(
        decode_settings(br#"{"selectedProvider":7}"#).selected_provider,
        Grok
    );
}

/// Legacy per-provider flags become the graph set and the old keys are removed.
#[test]
fn test_per_provider_graph_flags_migrate() {
    let file = Arc::new(SettingsFile::from_bytes(
        br#"{"showGrokBarInMenuBar":false,"showClaudeBarInMenuBar":true,"showCursorBarInMenuBar":true}"#,
    ));
    let s = AppSettings::new(Arc::clone(&file));
    assert_eq!(
        s.snapshot().menu_bar_graph_providers,
        set_of(&[Cursor, Claude])
    );
    let map = file.snapshot();
    assert!(!map.contains_key("showGrokBarInMenuBar"));
    assert!(!map.contains_key("showClaudeBarInMenuBar"));
    assert_eq!(
        map["menuBarGraphProviders"],
        serde_json::json!(["claude", "cursor"])
    );
}

/// With no flags and no key the graph set is Grok only; an existing key skips migration.
#[test]
fn test_graph_set_defaults_to_grok_and_existing_key_wins() {
    assert_eq!(
        AppSettings::new(Arc::new(SettingsFile::in_memory()))
            .snapshot()
            .menu_bar_graph_providers,
        set_of(&[Grok])
    );
    let file = Arc::new(SettingsFile::from_bytes(
        br#"{"menuBarGraphProviders":["claude","overview","openrouter"],"showGrokBarInMenuBar":true}"#,
    ));
    let s = AppSettings::new(Arc::clone(&file));
    assert_eq!(s.snapshot().menu_bar_graph_providers, set_of(&[Claude]));
    assert!(file.snapshot().contains_key("showGrokBarInMenuBar"));
}

/// Subscribers are notified on a real change and not on a no-op write.
#[test]
fn test_change_notification_fires_only_on_change() {
    let s = AppSettings::new(Arc::new(SettingsFile::in_memory()));
    let mut rx = s.subscribe();
    s.set_checks_for_updates(true);
    assert!(!rx.has_changed().expect("open"));
    s.set_checks_for_updates(false);
    assert!(rx.has_changed().expect("open"));
    assert!(!rx.borrow_and_update().checks_for_updates);
    s.set_active_poll_seconds(15);
    s.set_active_poll_seconds(10);
    assert!(rx.has_changed().expect("open"));
}

/// Settings keep unrelated keys (notifier records) when they are rewritten.
#[test]
fn test_unrelated_keys_are_preserved() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"legacyURLCacheRemoved":true,"thresholdNotified.x":80}"#,
    )
    .expect("seed");
    let s = AppSettings::open(&path);
    s.set_checks_for_updates(false);
    let text = std::fs::read_to_string(&path).expect("file");
    assert!(text.contains("legacyURLCacheRemoved") && text.contains("thresholdNotified.x"));
}

/// A corrupt file on disk loads as defaults and is repaired on the next write.
#[test]
fn test_corrupt_file_loads_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, b"{\"activePollSeconds\": 3").expect("seed");
    let s = AppSettings::open(&path);
    assert_eq!(s.snapshot(), Settings::default());
    s.set_checks_for_updates(false);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).expect("file")).is_ok()
    );
}
