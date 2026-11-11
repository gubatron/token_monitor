//! Ports of the Swift UI-kit tests (SegmentedUsageBarTests,
//! DailyBudgetHoverTests, ColorPaletteTests, DailyBudgetWeekOffsetTests) plus
//! view-model behavior of the daily charts.
#![allow(missing_docs)]

use chrono::NaiveDate;
use tokenmon_core::{ProviderId, Srgb};
use tokenmon_ui::kit::daily::{
    DailyBudgetDay, DayTokensText, day_bar, footer_text, header_percent, range_label, weekday_label,
};
use tokenmon_ui::kit::hover::{DayUsage, column_index, label_origin, usage_line};
use tokenmon_ui::kit::palette::{ProductColor, to_color};
use tokenmon_ui::kit::segments::{ProductShare, layout_segments, segment_widths};
use tokenmon_ui::kit::week::WeekOffset;

fn d(m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2025, m, day).expect("valid date")
}

fn day(date: NaiveDate, spent: f64, budget: f64) -> DailyBudgetDay {
    DailyBudgetDay {
        date,
        spent,
        budget,
        percent_of_budget: if budget > 0.0 {
            spent / budget * 100.0
        } else {
            0.0
        },
        is_over_budget: spent > budget,
        is_prior_window: false,
        tokens: None,
    }
}

/// testWidthsAreProportionalWhenTheyFit.
#[test]
fn test_segment_widths_are_proportional_when_they_fit() {
    assert_eq!(
        segment_widths(&[50.0, 25.0, 25.0], 100.0, 3.0),
        vec![50.0, 25.0, 25.0]
    );
}

/// testFloorsAreNormalizedSoTheyDoNotOverflow.
#[test]
fn test_segment_floors_are_normalized_to_usable_width() {
    let sum: f64 = segment_widths(&[0.1, 0.1, 0.1], 5.0, 3.0).iter().sum();
    assert!((sum - 5.0).abs() < 1e-4, "sum was {sum}");
}

/// testZeroUsableKeepsFloors.
#[test]
fn test_zero_usable_width_keeps_floors() {
    assert_eq!(segment_widths(&[50.0, 50.0], 0.0, 3.0), vec![3.0, 3.0]);
}

/// Hidden tiny products and the remainder slot follow the spec thresholds.
#[test]
fn test_layout_hides_tiny_products_and_adds_remainder() {
    let share = |p| ProductShare {
        name: "x".into(),
        percent_of_pool: p,
        color: ProductColor::Chat.srgb(),
    };
    let grey = Srgb::rgb(0.5, 0.5, 0.5);
    let specs = layout_segments(&[share(40.0), share(0.04), share(10.0)], 102.0, grey);
    // 2 visible + remainder (50%) = 3 slots, 2 gaps of 2 -> usable 98.
    assert_eq!(specs.len(), 3);
    assert!(specs[2].remainder);
    assert!((specs[0].width - 39.2).abs() < 1e-9);
    assert!((specs[2].width - 49.0).abs() < 1e-9);
    // Full pool -> no remainder slot.
    let full = layout_segments(&[share(60.0), share(40.0)], 100.0, grey);
    assert_eq!(full.len(), 2);
    assert!(full.iter().all(|s| !s.remainder));
}

/// testColumnIndexMapsThePointerToABar.
#[test]
fn test_column_index_maps_pointer_to_bar() {
    assert_eq!(column_index(0.0, 40.0, 7), Some(0));
    assert_eq!(column_index(119.9, 40.0, 7), Some(2));
    assert_eq!(column_index(279.0, 40.0, 7), Some(6));
    assert_eq!(column_index(280.0, 40.0, 7), None);
    assert_eq!(column_index(-1.0, 40.0, 7), None);
    assert_eq!(column_index(10.0, 0.0, 0), None);
}

/// testLabelFloatsBesideThePointer.
#[test]
fn test_label_floats_beside_pointer() {
    assert_eq!(
        label_origin((200.0, 50.0), (100.0, 60.0), 300.0),
        (86.0, 20.0)
    );
    assert_eq!(
        label_origin((40.0, 50.0), (100.0, 60.0), 300.0),
        (54.0, 20.0)
    );
    assert_eq!(
        label_origin((100.0, 50.0), (250.0, 60.0), 300.0),
        (0.0, 20.0)
    );
}

/// testUsageLineDescribesTheDay, plus the other branches of the spec.
#[test]
fn test_usage_line_describes_the_day() {
    let budget = 100.0 / 7.0;
    let usage = |spent, budget, prior| DayUsage {
        spent,
        budget,
        is_prior_window: prior,
    };
    assert_eq!(
        usage_line(usage(31.0, budget, false), "weekly"),
        "31.0% of weekly · 217% of daily allowance"
    );
    assert_eq!(
        usage_line(usage(0.0, budget, false), "weekly"),
        "0% of the 14.3% daily allowance"
    );
    assert_eq!(usage_line(usage(5.0, 0.0, false), "weekly"), "No budget");
    assert_eq!(
        usage_line(usage(12.34, budget, true), "monthly"),
        "12.3% of the monthly pool before the provider reset it early"
    );
}

/// testSRGBComponentsRoundTrip / testProductColorSRGB.
#[test]
fn test_palette_colors_match_spec() {
    let c = to_color(Srgb {
        red: 0.5,
        green: 0.25,
        blue: 0.75,
        alpha: 0.4,
    });
    assert_eq!(
        (c.red(), c.green(), c.blue(), c.alpha()),
        (128, 64, 191, 102)
    );
    assert!((ProductColor::Build.srgb().blue - 1.0).abs() < 1e-9);
    assert!((ProductColor::Chat.srgb().red - 0.11).abs() < 1e-9);
    assert_eq!(
        ProductColor::from_product_id("Grok-Build"),
        ProductColor::Build
    );
    assert_eq!(ProductColor::from_product_id("TEXT"), ProductColor::Chat);
    assert_eq!(
        ProductColor::from_product_id("media"),
        ProductColor::Imagine
    );
    assert_eq!(ProductColor::from_product_id("voice"), ProductColor::Voice);
    assert_eq!(ProductColor::from_product_id("api"), ProductColor::Api);
    assert_eq!(ProductColor::from_product_id("???"), ProductColor::Other);
}

/// testWeekOffsetNeverMovesPastTheCurrentWeek.
#[test]
fn test_week_offset_never_moves_past_current_week() {
    assert_eq!(WeekOffset(0).previous(), WeekOffset(-1));
    assert_eq!(WeekOffset(-2).previous(), WeekOffset(-3));
    assert_eq!(WeekOffset(0).next(), WeekOffset(0));
    assert_eq!(WeekOffset(-1).next(), WeekOffset(0));
    assert_eq!(WeekOffset(-4).next(), WeekOffset(-3));
    assert!(!WeekOffset(0).can_go_next());
    assert!(WeekOffset(-1).can_go_next());
    assert!(WeekOffset(0).shows_live_footer());
    assert!(!WeekOffset(-1).shows_live_footer());
}

/// Range label formats within and across months (spec examples).
#[test]
fn test_range_label_examples() {
    assert_eq!(range_label(d(10, 13), d(10, 19)), "October 13 – 19");
    assert_eq!(range_label(d(9, 29), d(10, 5)), "September 29 – Oct 5");
}

/// Header percent, weekday label and day title formatting.
#[test]
fn test_header_and_labels() {
    assert_eq!(header_percent(100.0 / 7.0), "14.3%");
    assert_eq!(header_percent(0.0), "");
    assert_eq!(weekday_label(d(10, 13)), "Mo");
    let bar = day_bar(&day(d(10, 14), 31.0, 100.0 / 7.0), d(10, 16), "weekly");
    assert_eq!(bar.hover.title.as_str(), "Tuesday, Oct 14");
    assert_eq!(bar.label_percent.as_str(), "31%");
    assert_eq!(bar.label_weekday.as_str(), "Tu");
    assert!(bar.show_fill && !bar.future);
}

/// Future days draw no fill and have no usage line; tiny usage draws nothing.
#[test]
fn test_future_and_tiny_days_draw_no_fill() {
    let today = d(10, 16);
    let future = day_bar(&day(d(10, 18), 5.0, 14.3), today, "weekly");
    assert!(future.future && !future.show_fill);
    assert_eq!(future.hover.caption.as_str(), "");
    let tiny = day_bar(&day(today, 0.01, 14.3), today, "weekly");
    assert!(!tiny.show_fill);
}

/// Token details only reach the hover when present; models cap at three.
#[test]
fn test_hover_tokens_and_model_cap() {
    use slint::Model;
    let mut dd = day(d(10, 14), 10.0, 14.3);
    dd.tokens = Some(DayTokensText {
        total: "1.2M".into(),
        cost: "≈ $4.20+".into(),
        input: "1M".into(),
        output: "100K".into(),
        cache_read: "90K".into(),
        cache_write: "10K".into(),
        models: (0..5).map(|i| (format!("m{i}"), "1K".into())).collect(),
    });
    let bar = day_bar(&dd, d(10, 16), "weekly");
    assert_eq!(bar.hover.total.as_str(), "1.2M");
    assert_eq!(bar.hover.rows.row_count(), 4);
    assert_eq!(bar.hover.extra_rows.row_count(), 3);
    let none = day_bar(&day(d(10, 14), 10.0, 14.3), d(10, 16), "weekly");
    assert_eq!(none.hover.total.as_str(), "");
}

/// Footer selection: pace caption wins, then "no usage", then over-budget.
#[test]
fn test_footer_selection() {
    let today = d(10, 14);
    let idle: Vec<_> = (13..=19).map(|n| day(d(10, n), 0.0, 100.0 / 7.0)).collect();
    assert_eq!(
        footer_text(
            &idle,
            today,
            Some("Usage 3.2% over today's allowance".into()),
            true
        ),
        "Usage 3.2% over today's allowance"
    );
    assert_eq!(
        footer_text(&idle, today, None, true),
        "No usage yet · 14.3%/day"
    );
    assert_eq!(footer_text(&idle, today, None, false), "");
    let mut busy = idle.clone();
    busy[1] = day(d(10, 14), 40.0, 100.0 / 7.0);
    assert_eq!(
        footer_text(&busy, today, None, true),
        "Over today's allowance"
    );
    busy[1] = day(d(10, 14), 5.0, 100.0 / 7.0);
    assert_eq!(footer_text(&busy, today, None, true), "");
}

/// Every provider accent converts to its exact 8-bit value.
#[test]
fn test_provider_accents_convert_exactly() {
    for p in [ProviderId::Grok, ProviderId::Claude, ProviderId::OpenRouter] {
        let c = to_color(p.accent());
        assert_eq!((c.red(), c.green(), c.blue()), p.accent().to_rgb8());
        assert_eq!(to_color(p.panel_accent()).alpha(), 217);
    }
}
