//! Overview panel and provider switcher view models.
#![allow(missing_docs, clippy::unwrap_used, clippy::assert_is_empty)]

mod panels_common;

use chrono::{FixedOffset, TimeZone, Timelike, Utc};
use panels_common::*;
use tokenmon_core::ProviderId;
use tokenmon_ui::panels::opencode_openrouter::PURPLE;
use tokenmon_ui::panels::overview::*;
use tokenmon_ui::panels::switcher::*;

fn card(p: ProviderId) -> OverviewCardInput {
    OverviewCardInput {
        provider: p,
        has_snapshot: false,
        percent: None,
        reset_at: None,
        signed_in: true,
        needs_sign_in: false,
        key_usage_usd: None,
        has_budget_source: false,
        remaining_usd: None,
    }
}

// -------------------------------- switcher ---------------------------------

/// ProviderSwitcherLayoutTests.testRowsPadTheLastRowToFullWidth for 1..8 tabs.
#[test]
fn test_rows_pad_the_last_row_to_full_width() {
    for n in 1..=8usize {
        let items: Vec<usize> = (0..n).collect();
        let r = rows(&items);
        assert_eq!(r.len(), n.div_ceil(4));
        assert!(r.iter().all(|row| row.len() == 4));
        let flat: Vec<usize> = r.into_iter().flatten().flatten().collect();
        assert_eq!(flat, items);
    }
}

/// Selection, labels, logos and dividers of the grid.
#[test]
fn test_switcher_labels_selection_and_dividers() {
    let vis = &ProviderId::USAGE_PROVIDERS[..];
    let g = build_switcher(vis, ProviderId::Cursor);
    assert_eq!(g.len(), 2);
    assert!(!g[0].top_divider && g[1].top_divider);
    assert_eq!(g[0].cells[0].tab.as_ref().unwrap().label, "All");
    assert!(g[0].cells[0].tab.as_ref().unwrap().logo.is_none());
    assert!(!g[0].cells[0].leading_divider && g[0].cells[1].leading_divider);
    let selected: Vec<_> = g
        .iter()
        .flat_map(|r| &r.cells)
        .filter_map(|c| c.tab.as_ref())
        .filter(|t| t.selected)
        .map(|t| t.provider)
        .collect();
    assert_eq!(selected, [ProviderId::Cursor]);
    // 8 tabs fill both rows completely.
    assert!(g[1].cells.iter().all(|c| c.tab.is_some()));
    // Row 2 of a 5-tab grid: one real tab then padding; divider after the real tab only.
    let g = build_switcher(&vis[..4], ProviderId::Overview);
    assert!(!g[1].cells[0].leading_divider);
    assert!(
        g[1].cells[1].leading_divider,
        "divider between last real tab and first empty cell"
    );
    assert!(!g[1].cells[2].leading_divider);
    assert!(g[1].cells[1].tab.is_none());
}

// -------------------------------- cards ------------------------------------

/// Grok card: percent, caption and sign-in/out rules.
#[test]
fn test_overview_grok_card() {
    let mut c = card(ProviderId::Grok);
    c.has_snapshot = true;
    c.percent = Some(35.5);
    c.reset_at = Some(at(2026, 8, 24, 0, 0));
    let v = build_card(&c, &ctx());
    assert_eq!((v.title.as_str(), v.meta), ("Grok", "— Weekly"));
    assert_eq!(v.used_text, "36% Used");
    assert_eq!(v.caption.as_deref(), Some("Resets Mon 24 August 12:00am"));
    assert!(v.sign_in.is_none());
    assert_eq!(v.sign_out, Some(ProviderId::Grok));
    c.needs_sign_in = true;
    let v = build_card(&c, &ctx());
    assert_eq!(v.sign_in.unwrap().0, "Sign In to Grok…");
    assert!(v.sign_out.is_none());
    let mut c = card(ProviderId::Grok);
    c.signed_in = false;
    let v = build_card(&c, &ctx());
    assert_eq!(v.caption.as_deref(), Some("No Grok data yet"));
    assert!(v.caption_is_tertiary);
    assert!(v.sign_in.is_some());
}

/// Metas and accent colors for every provider card.
#[test]
fn test_overview_meta_labels_and_colors() {
    let want = [
        (ProviderId::OpenCode, "— Monthly"),
        (ProviderId::Cursor, "— Monthly"),
        (ProviderId::Claude, "— Weekly"),
        (ProviderId::ChatGpt, "— Codex"),
        (ProviderId::Grokbot, "— Weekly"),
        (ProviderId::OpenRouter, "— Credits"),
    ];
    for (p, m) in want {
        let v = build_card(&card(p), &ctx());
        assert_eq!(v.meta, m);
        assert_eq!(
            v.caption.as_deref(),
            Some(format!("No {} data yet", p.display_name()).as_str())
        );
        assert_eq!(
            v.color,
            if p == ProviderId::OpenCode {
                PURPLE
            } else {
                p.panel_accent()
            }
        );
    }
}

/// Non-Grok cards: sign-in only without snapshot; sign-out when not needing sign-in.
#[test]
fn test_overview_sign_in_out_rules() {
    let mut c = card(ProviderId::Claude);
    c.needs_sign_in = true;
    let v = build_card(&c, &ctx());
    assert_eq!(
        v.sign_in.unwrap().1,
        CardAction::OpenSignIn(ProviderId::Claude)
    );
    assert!(v.sign_out.is_none());
    c.has_snapshot = true;
    assert!(build_card(&c, &ctx()).sign_in.is_none());
    c.needs_sign_in = false;
    assert_eq!(build_card(&c, &ctx()).sign_out, Some(ProviderId::Claude));
}

/// Grokbot signs in through Cursor and never shows sign-out.
#[test]
fn test_overview_grokbot_uses_cursor_sign_in() {
    let mut c = card(ProviderId::Grokbot);
    c.needs_sign_in = true;
    let v = build_card(&c, &ctx());
    let (label, action) = v.sign_in.unwrap();
    assert_eq!(label, "Sign In to Cursor…");
    assert_eq!(action, CardAction::OpenSignIn(ProviderId::Cursor));
    c.needs_sign_in = false;
    assert!(build_card(&c, &ctx()).sign_out.is_none());
}

/// OpenRouter captions, USD header and tab-select sign-in.
#[test]
fn test_overview_openrouter_card() {
    let mut c = card(ProviderId::OpenRouter);
    c.needs_sign_in = true;
    assert_eq!(
        build_card(&c, &ctx()).sign_in.unwrap().1,
        CardAction::SelectTab(ProviderId::OpenRouter)
    );
    let mut c = card(ProviderId::OpenRouter);
    c.has_snapshot = true;
    c.key_usage_usd = Some(12.34);
    let v = build_card(&c, &ctx());
    assert_eq!(v.used_text, "$12.34");
    assert_eq!(
        v.caption.as_deref(),
        Some("No credit limit — $12.34 spent on this key")
    );
    assert!(v.caption_is_tertiary);
    c.has_budget_source = true;
    c.percent = Some(50.0);
    c.remaining_usd = Some(5.5);
    let v = build_card(&c, &ctx());
    assert_eq!(v.used_text, "50% Used");
    assert_eq!(v.caption.as_deref(), Some("$5.50 of credits left"));
    assert_eq!(v.fraction, 0.5);
}

// ------------------------------- hourly chart ------------------------------

/// OverviewHourlyHistoryTests: segment heights.
#[test]
fn test_segment_heights() {
    assert_eq!(segment_heights(&[1.0, 3.0], 40.0, 1.0), vec![10.0, 30.0]);
    let h = segment_heights(&[0.001, 0.002, 10.0], 20.0, 1.0);
    assert!((h[0] - 1.0).abs() < 1e-9 && (h[1] - 1.0).abs() < 1e-9 && (h[2] - 18.0).abs() < 1e-6);
    assert!((h.iter().sum::<f64>() - 20.0).abs() < 1e-9);
    let h = segment_heights(&[1.0, 1.0, 2.0], 2.0, 1.0);
    assert!((h.iter().sum::<f64>() - 2.0).abs() < 1e-9);
    assert!((h[2] - 1.0).abs() < 1e-9);
    assert_eq!(segment_heights(&[0.0, 0.0], 10.0, 1.0), vec![0.0, 0.0]);
}

/// Bar height with visible minimum.
#[test]
fn test_bar_height() {
    assert_eq!(bar_height(5.0, 10.0, 60.0), 30.0);
    assert_eq!(bar_height(10.0, 10.0, 60.0), 60.0);
    assert_eq!(bar_height(0.001, 10.0, 60.0), 3.0);
    assert_eq!(bar_height(0.0, 10.0, 60.0), 0.0);
}

/// Visible range and axis labels.
#[test]
fn test_visible_hours_and_axis() {
    assert_eq!(visible_hour_range(&[10]), (6, 22));
    assert_eq!(visible_hour_range(&[]), (6, 22));
    assert_eq!(visible_hour_range(&[2, 23]), (2, 23));
    assert_eq!(axis_hours((6, 22)), [6, 9, 12, 15, 18, 22]);
    assert_eq!(axis_hours((2, 23)), [2, 6, 9, 12, 15, 18, 21, 23]);
    assert_eq!(axis_label(0), "12");
    assert_eq!(axis_label(12), "12");
    assert_eq!(axis_label(15), "3");
    assert_eq!(axis_label(9), "9");
    assert_eq!(hour_label(0), "12a");
    assert_eq!(hour_label(11), "11a");
    assert_eq!(hour_label(12), "12p");
    assert_eq!(hour_label(23), "11p");
}

fn hours_with(acts: &[(usize, HourSeries, f64, Option<u64>)]) -> Vec<Vec<HourActivityInput>> {
    let mut h = vec![Vec::new(); 24];
    for (hr, series, q, t) in acts {
        h[*hr].push(HourActivityInput {
            series: *series,
            quota_percent: *q,
            tokens: *t,
        });
    }
    h
}

/// Chart columns, tooltip, legend order and axis labels.
#[test]
fn test_hourly_chart_columns_tooltip_legend() {
    let hours = hours_with(&[
        (9, HourSeries::Claude, 2.0, Some(1_500_000)),
        (9, HourSeries::Grok, 1.0, Some(500)),
        (9, HourSeries::Grokbot, 1.0, Some(77)),
        (10, HourSeries::OpenCodeZen, 0.5, None),
    ]);
    let c = build_hourly_chart(&hours, true);
    assert!(c.empty_message.is_none());
    assert_eq!(c.columns.len(), 17, "6..=22");
    let col9 = c.columns.iter().find(|c| c.hour == 9).unwrap();
    assert_eq!(col9.bar_height, 68.0);
    assert!((col9.segments.iter().map(|s| s.height).sum::<f64>() - 68.0).abs() < 1e-9);
    assert_eq!(
        col9.tooltip,
        "9a: Grok 1.00% of weekly · 500 · Grokbot 1.00% of weekly · Claude 2.00% of 5-hour window · 1.5M tokens on this PC"
    );
    assert_eq!(col9.axis_label.as_deref(), Some("9"));
    let col10 = c.columns.iter().find(|c| c.hour == 10).unwrap();
    assert_eq!(col10.bar_height, 68.0 * 0.5 / 4.0);
    let idle = c.columns.iter().find(|c| c.hour == 7).unwrap();
    assert_eq!(idle.bar_height, 0.0);
    assert!(idle.tooltip.starts_with("7a:"));
    let legend: Vec<&str> = c.legend.iter().map(|l| l.title).collect();
    assert_eq!(legend, ["Grok", "Grokbot", "Claude", "Zen"]);
}

/// Empty messages for today and past days.
#[test]
fn test_hourly_chart_empty_messages() {
    let empty = vec![Vec::new(); 24];
    assert_eq!(
        build_hourly_chart(&empty, true).empty_message,
        Some("No Grok, Grokbot, OpenCode, Cursor, or Claude activity yet today.")
    );
    assert_eq!(
        build_hourly_chart(&empty, false).empty_message,
        Some("No activity recorded on this day.")
    );
}

/// Day titles, back-arrow rule and midnight schedule.
#[test]
fn test_day_titles_back_arrow_and_midnights() {
    let d = day(2026, 10, 20);
    assert_eq!(day_title(0, d), "Usage Today");
    assert_eq!(day_title(-1, d), "Usage Yesterday");
    assert_eq!(day_title(-2, d), "Usage Tue, Oct 20");
    assert!(!can_go_back(d, None));
    assert!(!can_go_back(d, Some(d)));
    assert!(can_go_back(d, Some(day(2026, 10, 19))));

    let tz = FixedOffset::west_opt(6 * 3600).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 3, 8, 5, 59, 0).unwrap(); // 03-07 23:59 local
    let s = midnight_schedule(now, tz);
    assert_eq!(s.len(), 8);
    assert!(s.iter().all(|t| t.with_timezone(&tz).hour() == 0));
    assert_eq!(s[0].with_timezone(&tz).date_naive(), day(2026, 3, 7));
    assert_eq!(s[1].with_timezone(&tz).date_naive(), day(2026, 3, 8));
}

/// Whole Overview panel: version default, navigation flags.
#[test]
fn test_overview_panel_header_and_navigation() {
    let mut input = OverviewInput {
        version: None,
        cards: vec![card(ProviderId::Grok)],
        daily: OverviewDailyInput {
            day_offset: 0,
            hours: vec![Vec::new(); 24],
            earliest_day: None,
        },
    };
    let v = build_overview(&input, &ctx());
    assert_eq!((v.title, v.version_text.as_str()), ("TokenMon", "v1.0.0"));
    assert!(!v.daily.has_navigation && !v.daily.can_go_forward);
    input.version = Some("1.10.1".into());
    input.daily.earliest_day = Some(day(2026, 7, 1));
    let v = build_overview(&input, &ctx());
    assert_eq!(v.version_text, "v1.10.1");
    assert!(v.daily.has_navigation && v.daily.can_go_back);
    input.daily.day_offset = -1;
    let v = build_overview(&input, &ctx());
    assert_eq!(v.daily.title, "Usage Yesterday");
    assert!(v.daily.can_go_forward);
    assert_eq!(
        v.daily.chart.empty_message,
        Some("No activity recorded on this day.")
    );
}

/// `tab_provider` resolves occupied cells and rejects padding and
/// out-of-range coordinates instead of panicking.
#[test]
fn test_tab_provider_resolves_cells_and_rejects_invalid_coordinates() {
    let rows = build_switcher(&[ProviderId::Grok], ProviderId::Overview);
    assert_eq!(tab_provider(&rows, 0, 0), Some(ProviderId::Overview));
    assert_eq!(tab_provider(&rows, 0, 1), Some(ProviderId::Grok));
    assert_eq!(tab_provider(&rows, 0, 2), None, "padding cell");
    assert_eq!(tab_provider(&rows, 0, 99), None, "column out of range");
    assert_eq!(tab_provider(&rows, 5, 0), None, "row out of range");
}
