//! Contract tests for ProviderId.

use tokenmon_core::ProviderId::{self, *};
use tokenmon_core::{PollIntervalInputs, select_poll_interval};

use std::time::Duration;

/// Every variant survives `Display` then `FromStr`.
#[test]
fn test_provider_id_display_fromstr_roundtrip() {
    for p in ProviderId::ALL_CASES {
        assert_eq!(p.to_string().parse::<ProviderId>().unwrap(), p);
    }
}

/// Unknown and differently-cased ids are rejected.
#[test]
fn test_provider_id_rejects_invalid_input() {
    assert!("nonexistent".parse::<ProviderId>().is_err());
    assert!("Grok".parse::<ProviderId>().is_err());
    assert!("".parse::<ProviderId>().is_err());
}

/// Serde uses the persisted lowercase id.
#[test]
fn test_provider_id_serde_uses_persisted_ids() {
    assert_eq!(serde_json::to_string(&OpenCode).unwrap(), "\"opencode\"");
    assert_eq!(serde_json::to_string(&ChatGpt).unwrap(), "\"chatgpt\"");
    for p in ProviderId::ALL_CASES {
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, format!("\"{p}\""));
        assert_eq!(serde_json::from_str::<ProviderId>(&json).unwrap(), p);
    }
}

/// The two static orders match spec 04 section 1.1.
#[test]
fn test_provider_orders_match_spec() {
    assert_eq!(
        ProviderId::ALL_CASES,
        [
            Overview, Grok, Cursor, OpenCode, Claude, ChatGpt, OpenRouter, Grokbot
        ]
    );
    assert_eq!(
        ProviderId::USAGE_PROVIDERS,
        [Grok, Cursor, OpenCode, Claude, ChatGpt, OpenRouter, Grokbot]
    );
    assert_eq!(
        ProviderId::MENU_BAR_GRAPH_PROVIDERS,
        [Grok, Grokbot, Claude, Cursor, OpenCode]
    );
}

/// Graph support flags agree with the `MENU_BAR_GRAPH_PROVIDERS` list.
#[test]
fn test_menu_bar_graph_support_matches_list() {
    for p in ProviderId::ALL_CASES {
        assert_eq!(
            p.supports_menu_bar_graph(),
            ProviderId::MENU_BAR_GRAPH_PROVIDERS.contains(&p),
            "{p}"
        );
    }
    assert!(!ChatGpt.supports_menu_bar_graph());
    assert!(!OpenRouter.supports_menu_bar_graph());
}

/// Names, switcher labels and logo keys.
#[test]
fn test_names_labels_and_logo_keys() {
    assert_eq!(Overview.display_name(), "Overview");
    assert_eq!(Overview.switcher_label(), "All");
    assert_eq!(ChatGpt.display_name(), "ChatGPT");
    assert_eq!(Overview.logo_asset_key(), None);
    for p in ProviderId::USAGE_PROVIDERS {
        assert!(p.logo_asset_key().is_some(), "{p}");
    }
    assert_eq!(Claude.logo_asset_key(), Some("anthropic"));
}

/// Grokbot shares Cursor's session; OpenRouter and Overview have no window.
#[test]
fn test_sign_in_mapping() {
    assert_eq!(Grokbot.sign_in_provider(), Cursor);
    for p in ProviderId::ALL_CASES.into_iter().filter(|p| *p != Grokbot) {
        assert_eq!(p.sign_in_provider(), p);
    }
    assert!(!OpenRouter.has_sign_in_window());
    assert!(!Overview.has_sign_in_window());
    assert!(Grok.has_sign_in_window());
}

/// Accent floats are the exact spec values and the panel alpha is 0.85.
#[test]
fn test_accent_colors_are_exact() {
    let expect = [
        (Overview, 0.50, 0.50, 0.50),
        (Grok, 0.11, 0.38, 0.82),
        (OpenCode, 1.00, 0.55, 0.00),
        (Cursor, 0.18, 0.53, 0.38),
        (Claude, 0.85, 0.47, 0.34),
        (ChatGpt, 0.16, 0.52, 0.46),
        (OpenRouter, 0.45, 0.36, 0.90),
        (Grokbot, 0.38, 0.42, 0.50),
    ];
    for (p, r, g, b) in expect {
        let c = p.accent();
        assert_eq!((c.red, c.green, c.blue, c.alpha), (r, g, b, 1.0), "{p}");
        assert_eq!(p.panel_accent().alpha, 0.85);
    }
}

/// 8-bit conversion matches the spec hex values.
#[test]
fn test_accent_rgb8_matches_spec_hex() {
    assert_eq!(Grok.accent().to_rgb8(), (0x1C, 0x61, 0xD1));
    assert_eq!(OpenCode.accent().to_rgb8(), (0xFF, 0x8C, 0x00));
    assert_eq!(Cursor.accent().to_rgb8(), (0x2E, 0x87, 0x61));
    assert_eq!(Claude.accent().to_rgb8(), (0xD9, 0x78, 0x57));
    assert_eq!(ChatGpt.accent().to_rgb8(), (0x29, 0x85, 0x75));
    assert_eq!(OpenRouter.accent().to_rgb8(), (0x73, 0x5C, 0xE6));
    assert_eq!(Grokbot.accent().to_rgb8(), (0x61, 0x6B, 0x80));
    assert_eq!(Overview.accent().to_rgb8(), (0x80, 0x80, 0x80));
}

/// `normalizedOrder` drops overview/duplicates and appends the missing ones.
#[test]
fn test_normalized_order() {
    assert_eq!(
        ProviderId::normalized_order(&[]),
        ProviderId::USAGE_PROVIDERS
    );
    assert_eq!(
        ProviderId::normalized_order(&[Claude, Overview, Claude, Grok]),
        [Claude, Grok, Cursor, OpenCode, ChatGpt, OpenRouter, Grokbot]
    );
    assert_eq!(
        ProviderId::normalized_order(&[
            Grokbot, OpenRouter, ChatGpt, Claude, OpenCode, Cursor, Grok
        ]),
        [Grokbot, OpenRouter, ChatGpt, Claude, OpenCode, Cursor, Grok]
    );
}

/// `polls`: a mode polls itself and everything when Overview.
#[test]
fn test_polls() {
    assert!(Overview.polls(Claude));
    assert!(Claude.polls(Claude));
    assert!(!Claude.polls(Grok));
}

/// Poll interval is `None` unless polling is needed; picks active vs idle.
#[test]
fn test_select_poll_interval() {
    let base = PollIntervalInputs {
        needs_polling: true,
        menu_open: false,
        active_seconds: 60.0,
        idle_seconds: 300.0,
    };
    assert_eq!(select_poll_interval(base), Some(Duration::from_secs(300)));
    assert_eq!(
        select_poll_interval(PollIntervalInputs {
            menu_open: true,
            ..base
        }),
        Some(Duration::from_secs(60))
    );
    assert_eq!(
        select_poll_interval(PollIntervalInputs {
            needs_polling: false,
            ..base
        }),
        None
    );
    assert_eq!(
        select_poll_interval(PollIntervalInputs {
            idle_seconds: 0.0,
            ..base
        }),
        None
    );
    assert_eq!(
        select_poll_interval(PollIntervalInputs {
            idle_seconds: f64::NAN,
            ..base
        }),
        None
    );
}
