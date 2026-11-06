//! Tray icon renderer, tooltip and menu tests (spec 04 section 2.7).
//!
//! PNG snapshots live in `tests/snapshots/`; regenerate with
//! `UPDATE_SNAPSHOTS=1 cargo test -p tokenmon-ui --test tray_icon`.

#![allow(clippy::chunks_exact_to_as_chunks)]

use std::path::PathBuf;

use tokenmon_core::ProviderId::{self, *};
use tokenmon_ui::tray_icon::*;

const ALL_SIZES: [TrayIconSize; 5] = [
    TrayIconSize::Px16,
    TrayIconSize::Px20,
    TrayIconSize::Px24,
    TrayIconSize::Px32,
    TrayIconSize::Px40,
];

fn req(content: TrayContent, size: TrayIconSize, theme: TaskbarTheme) -> TrayIconRequest {
    TrayIconRequest {
        content,
        size,
        theme,
    }
}

fn selected(p: ProviderId, used: Option<f64>) -> TrayContent {
    TrayContent::Selected { provider: p, used }
}

fn bars(items: &[(ProviderId, Option<f64>)]) -> TrayContent {
    TrayContent::Bars(
        items
            .iter()
            .map(|&(provider, used)| BarSegment { provider, used })
            .collect(),
    )
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.png"))
}

/// Compares the rendered icon against its golden PNG within +-2 per channel
/// (SIMD and scalar rasterizers may differ by rounding).
fn assert_snapshot(name: &str, r: &TrayIconRequest) {
    let bmp = render_tray_icon(r);
    let path = snapshot_path(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, bmp.to_png()).expect("write snapshot");
        return;
    }
    let golden = std::fs::read(&path).unwrap_or_else(|_| panic!("missing snapshot {name}"));
    let pm = tiny_skia::Pixmap::decode_png(&golden).expect("decode golden");
    assert_eq!(pm.width(), bmp.size, "{name}: size");
    let golden_rgba: Vec<u8> = pm
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    let worst = golden_rgba
        .iter()
        .zip(&bmp.rgba)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0);
    assert!(worst <= 2, "{name}: differs from golden by {worst}");
}

fn opaque_pixels(b: &TrayBitmap) -> usize {
    b.rgba.chunks_exact(4).filter(|p| p[3] > 0).count()
}

/// Selected-provider icon snapshots across every DPI size, dark taskbar.
#[test]
fn test_selected_icon_snapshots_all_sizes_dark() {
    for size in ALL_SIZES {
        let name = format!("selected_claude63_dark_{}", size.pixels());
        assert_snapshot(
            &name,
            &req(selected(Claude, Some(63.0)), size, TaskbarTheme::Dark),
        );
    }
}

/// Light-taskbar variants, plus 3-digit, no-data and 0% cases.
#[test]
fn test_selected_icon_snapshots_light_and_edge_values() {
    use TaskbarTheme::{Dark, Light};
    let cases = [
        (
            "selected_grok41_light_16",
            selected(Grok, Some(41.0)),
            TrayIconSize::Px16,
            Light,
        ),
        (
            "selected_grok41_light_32",
            selected(Grok, Some(41.0)),
            TrayIconSize::Px32,
            Light,
        ),
        (
            "selected_cursor100_dark_32",
            selected(Cursor, Some(100.0)),
            TrayIconSize::Px32,
            Dark,
        ),
        (
            "selected_opencode_nodata_dark_24",
            selected(OpenCode, None),
            TrayIconSize::Px24,
            Dark,
        ),
        (
            "selected_chatgpt0_light_40",
            selected(ChatGpt, Some(0.0)),
            TrayIconSize::Px40,
            Light,
        ),
    ];
    for (name, content, size, theme) in cases {
        assert_snapshot(name, &req(content, size, theme));
    }
}

/// Overview / composite bars and the empty state, both themes.
#[test]
fn test_bars_and_empty_snapshots() {
    let four = bars(&[
        (Grok, Some(41.0)),
        (Cursor, Some(12.0)),
        (OpenCode, None),
        (Claude, Some(63.0)),
    ]);
    for (theme, t) in [(TaskbarTheme::Dark, "dark"), (TaskbarTheme::Light, "light")] {
        for size in [TrayIconSize::Px16, TrayIconSize::Px32] {
            assert_snapshot(
                &format!("bars4_{t}_{}", size.pixels()),
                &req(four.clone(), size, theme),
            );
            assert_snapshot(
                &format!("empty_{t}_{}", size.pixels()),
                &req(TrayContent::Empty, size, theme),
            );
        }
    }
    assert_snapshot(
        "bars1_grok_dark_20",
        &req(
            bars(&[(Grok, Some(80.0))]),
            TrayIconSize::Px20,
            TaskbarTheme::Dark,
        ),
    );
}

/// Chrome is black on a light taskbar and white on a dark one.
#[test]
fn test_theme_changes_chrome_color() {
    let c = TrayContent::Empty;
    let dark = render_tray_icon(&req(c.clone(), TrayIconSize::Px32, TaskbarTheme::Dark));
    let light = render_tray_icon(&req(c, TrayIconSize::Px32, TaskbarTheme::Light));
    assert_ne!(dark, light);
    assert!(
        dark.rgba
            .chunks_exact(4)
            .any(|p| p[3] == 255 && p[0] == 255)
    );
    assert!(light.rgba.chunks_exact(4).any(|p| p[3] == 255 && p[0] == 0));
}

/// Bar fill grows with the percentage but a 2 px sliver is shown at 0%.
#[test]
fn test_bar_fill_monotonic_with_sliver_at_zero() {
    let area = |u: Option<f64>| {
        opaque_pixels(&render_tray_icon(&req(
            bars(&[(Grok, u)]),
            TrayIconSize::Px32,
            TaskbarTheme::Dark,
        )))
    };
    let accent = |u: Option<f64>| {
        let b = render_tray_icon(&req(
            bars(&[(Grok, u)]),
            TrayIconSize::Px32,
            TaskbarTheme::Dark,
        ));
        b.rgba
            .chunks_exact(4)
            .filter(|p| p[3] == 255 && p[2] > p[0])
            .count()
    };
    assert!(accent(Some(0.0)) > 0, "sliver must be visible at 0%");
    assert!(accent(Some(25.0)) < accent(Some(75.0)));
    assert!(accent(Some(75.0)) < accent(Some(100.0)));
    assert_eq!(accent(None), accent(Some(0.0)));
    assert!(area(Some(50.0)) > 0);
}

/// Percent digits appear only at >= 32 px (more opaque pixels than without).
#[test]
fn test_digits_only_at_32px_and_up() {
    let count = |s, u| {
        opaque_pixels(&render_tray_icon(&req(
            selected(Claude, u),
            s,
            TaskbarTheme::Dark,
        )))
    };
    assert_eq!(
        count(TrayIconSize::Px24, Some(63.0)),
        count(TrayIconSize::Px24, Some(64.0)),
        "24 px draws no digits so the value cannot change the glyph area"
    );
    assert_ne!(
        count(TrayIconSize::Px32, Some(63.0)),
        count(TrayIconSize::Px32, None)
    );
}

/// Every provider, size and theme renders a full-size, non-blank bitmap.
#[test]
fn test_all_providers_all_sizes_render_nonblank() {
    for p in ProviderId::USAGE_PROVIDERS {
        for size in ALL_SIZES {
            for theme in [TaskbarTheme::Light, TaskbarTheme::Dark] {
                let b = render_tray_icon(&req(selected(p, Some(50.0)), size, theme));
                let n = size.pixels() as usize;
                assert_eq!(b.rgba.len(), n * n * 4);
                assert!(opaque_pixels(&b) > n, "{p} {n}px blank");
            }
        }
    }
}

/// Out-of-range and NaN percentages are clamped, never panic.
#[test]
fn test_out_of_range_percentages_are_safe() {
    for u in [-50.0, 250.0, f64::NAN, f64::INFINITY] {
        let _ = render_tray_icon(&req(
            selected(Grok, Some(u)),
            TrayIconSize::Px32,
            TaskbarTheme::Dark,
        ));
        let _ = render_tray_icon(&req(
            bars(&[(Grok, Some(u))]),
            TrayIconSize::Px16,
            TaskbarTheme::Dark,
        ));
    }
}

/// BGRA premultiplied conversion swaps channels and premultiplies.
#[test]
fn test_bgra_premultiplied_conversion() {
    let b = TrayBitmap {
        size: 1,
        rgba: vec![200, 100, 50, 128],
    };
    assert_eq!(b.to_bgra_premultiplied(), vec![25, 50, 100, 128]);
}

/// The cache key changes with theme, size, rounded percent and content.
#[test]
fn test_cache_key_includes_every_input() {
    let base = req(
        selected(Grok, Some(41.2)),
        TrayIconSize::Px16,
        TaskbarTheme::Dark,
    );
    let same_rounded = req(
        selected(Grok, Some(40.9)),
        TrayIconSize::Px16,
        TaskbarTheme::Dark,
    );
    assert_eq!(base.key(), same_rounded.key());
    let variants = [
        req(
            selected(Grok, Some(41.2)),
            TrayIconSize::Px16,
            TaskbarTheme::Light,
        ),
        req(
            selected(Grok, Some(41.2)),
            TrayIconSize::Px20,
            TaskbarTheme::Dark,
        ),
        req(
            selected(Grok, Some(42.0)),
            TrayIconSize::Px16,
            TaskbarTheme::Dark,
        ),
        req(
            selected(Claude, Some(41.2)),
            TrayIconSize::Px16,
            TaskbarTheme::Dark,
        ),
        req(selected(Grok, None), TrayIconSize::Px16, TaskbarTheme::Dark),
        req(
            bars(&[(Grok, Some(41.2))]),
            TrayIconSize::Px16,
            TaskbarTheme::Dark,
        ),
    ];
    for v in variants {
        assert_ne!(base.key(), v.key(), "{}", v.key());
    }
}

/// Re-requesting the same key returns the same allocation; LRU caps at 40.
#[test]
fn test_cache_hits_and_lru_eviction() {
    let mut cache = TrayIconCache::new();
    let r = req(
        selected(Grok, Some(10.0)),
        TrayIconSize::Px16,
        TaskbarTheme::Dark,
    );
    let a = cache.get_or_render(&r);
    let b = cache.get_or_render(&r);
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    for pct in 0..100 {
        cache.get_or_render(&req(
            selected(Cursor, Some(f64::from(pct))),
            TrayIconSize::Px16,
            TaskbarTheme::Dark,
        ));
    }
    assert_eq!(cache.len(), TRAY_CACHE_CAPACITY);
    let c = cache.get_or_render(&r);
    assert!(!std::sync::Arc::ptr_eq(&a, &c), "evicted entry re-renders");
}

/// A theme flip with otherwise identical inputs misses the cache.
#[test]
fn test_cache_theme_flip_rerenders() {
    let mut cache = TrayIconCache::new();
    let dark = cache.get_or_render(&req(
        TrayContent::Empty,
        TrayIconSize::Px16,
        TaskbarTheme::Dark,
    ));
    let light = cache.get_or_render(&req(
        TrayContent::Empty,
        TrayIconSize::Px16,
        TaskbarTheme::Light,
    ));
    assert_ne!(dark, light);
    assert_eq!(cache.len(), 2);
}

/// DPI-derived pixel sizes snap to the nearest supported size.
#[test]
fn test_size_nearest() {
    assert_eq!(TrayIconSize::nearest(0), TrayIconSize::Px16);
    assert_eq!(TrayIconSize::nearest(18), TrayIconSize::Px20);
    assert_eq!(TrayIconSize::nearest(36), TrayIconSize::Px40);
}

/// Theme source trait and registry-value mapping.
#[test]
fn test_theme_source_and_registry_mapping() {
    let src: &dyn TaskbarThemeSource = &FixedThemeSource(TaskbarTheme::Light);
    assert_eq!(src.taskbar_theme(), TaskbarTheme::Light);
    assert_eq!(
        TaskbarTheme::from_system_uses_light_theme(Some(7)),
        TaskbarTheme::Light
    );
    assert_eq!(
        TaskbarTheme::from_system_uses_light_theme(None),
        TaskbarTheme::Dark
    );
}

fn inputs<'a>(
    selected: ProviderId,
    show_selected: bool,
    enabled: &'a [ProviderId],
    graphs: &'a [ProviderId],
    used: &'a [(ProviderId, f64)],
) -> TrayInputs<'a> {
    TrayInputs {
        selected,
        show_selected,
        used_percent: used,
        provider_order: &[],
        menu_bar_graphs: graphs,
        enabled,
        grok_signed_in: true,
    }
}

/// Overview or composite mode yields at most 4 bars in normalized order;
/// Grok always participates, others only with their graph on.
#[test]
fn test_content_composite_caps_at_four_and_follows_order() {
    let all = ProviderId::USAGE_PROVIDERS;
    let graphs = [Claude, Cursor, OpenCode, Grokbot, ChatGpt];
    let c = TrayContent::from_inputs(&inputs(Overview, true, &all, &graphs, &[(Grok, 41.0)]));
    let TrayContent::Bars(b) = c else {
        panic!("expected bars")
    };
    let order: Vec<_> = b.iter().map(|s| s.provider).collect();
    assert_eq!(order, [Grok, Cursor, OpenCode, Claude]);
    assert_eq!(b[0].used, Some(41.0));
    assert_eq!(b[1].used, None);
}

/// Grok signed out has no value; nothing enabled is Empty.
#[test]
fn test_content_grok_signed_out_and_empty() {
    let used = [(Grok, 50.0)];
    let mut i = inputs(Grok, true, &[Grok], &[], &used);
    i.grok_signed_in = false;
    assert_eq!(TrayContent::from_inputs(&i), selected(Grok, None));
    let none = inputs(Overview, false, &[], &[], &[]);
    assert_eq!(TrayContent::from_inputs(&none), TrayContent::Empty);
}

fn entry(provider: ProviderId, used: Option<f64>) -> TooltipEntry {
    TooltipEntry {
        provider,
        used,
        signed_out: false,
    }
}

/// Spec example: `Grok 41% · Cursor 12% · OpenCode — · Claude 63%`.
#[test]
fn test_tooltip_matches_spec_example() {
    let t = build_tooltip(&[
        entry(Grok, Some(41.0)),
        entry(Cursor, Some(12.0)),
        entry(OpenCode, None),
        entry(Claude, Some(63.0)),
    ]);
    assert_eq!(
        t,
        "Grok 41% \u{b7} Cursor 12% \u{b7} OpenCode \u{2014} \u{b7} Claude 63%"
    );
    let signed_out = TooltipEntry {
        provider: Grok,
        used: Some(5.0),
        signed_out: true,
    };
    assert_eq!(build_tooltip(&[signed_out]), "Grok");
}

/// Long tooltips drop whole trailing entries and stay within 127 UTF-16 units.
#[test]
fn test_tooltip_truncates_at_entry_boundary() {
    let many: Vec<_> = (0..20).map(|_| entry(OpenRouter, Some(100.0))).collect();
    let t = build_tooltip(&many);
    assert!(t.encode_utf16().count() <= TOOLTIP_MAX_UTF16);
    assert!(t.ends_with(" \u{2026}"));
    assert!(t.starts_with("OpenRouter 100%"));
    assert_eq!(build_tooltip(&[]), "");
}

/// The szTip buffer is NUL-terminated and never splits a surrogate pair.
#[test]
fn test_encode_tip_utf16_bounds() {
    let long: String = "\u{1F600}".repeat(100);
    let buf = encode_tip_utf16(&long);
    assert_eq!(buf[127], 0);
    let len = buf.iter().position(|&u| u == 0).unwrap_or(128);
    assert_eq!(len % 2, 0, "whole surrogate pairs only");
    assert!(String::from_utf16(&buf[..len]).is_ok());
}

/// Menu order: Open per enabled provider in order, Overview, Settings, updates, Quit.
#[test]
fn test_menu_structure_and_order() {
    let m = build_menu(&MenuInputs {
        provider_order: &[Claude, Grok],
        enabled: &[Grok, Claude, Cursor],
    });
    let labels: Vec<String> = m
        .iter()
        .map(|e| match e {
            MenuEntry::Item { label, .. } => label.clone(),
            MenuEntry::Separator => "---".into(),
        })
        .collect();
    assert_eq!(
        labels,
        [
            "Open Claude",
            "Open Grok",
            "Open Cursor",
            "Open Overview",
            "---",
            "Settings\u{2026}",
            "Check for Updates",
            "---",
            "Quit TokenMon",
        ]
    );
}

/// Menu action ids are unique and round-trip.
#[test]
fn test_menu_action_ids_roundtrip() {
    let mut actions = vec![
        MenuAction::OpenOverview,
        MenuAction::Settings,
        MenuAction::CheckForUpdates,
        MenuAction::Quit,
    ];
    actions.extend(ProviderId::USAGE_PROVIDERS.map(MenuAction::OpenProvider));
    let mut ids: Vec<u32> = actions.iter().map(|a| a.id()).collect();
    for (a, id) in actions.iter().zip(&ids) {
        assert_eq!(MenuAction::from_id(*id), Some(*a));
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), actions.len());
    assert_eq!(MenuAction::from_id(0), None);
    assert_eq!(MenuAction::from_id(107), None);
}
