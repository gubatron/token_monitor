//! Placement of the floating hover labels and nearest-row picking (Swift
//! `HoverLabelTests`, "Placement"), the shared value text of a day's label
//! and the token block (Swift "Daily label value").
#![allow(missing_docs, clippy::unwrap_used)]

use tokenmon_ui::kit::hover::{POINTER_GAP, RowFrame, label_origin_within, nearest_row_index};
use tokenmon_ui::panels::common::{
    AllowanceNoun, DailyBudgetInput, DayInput, FooterKind, TokensInput, build_daily_budget,
};

fn row(min_y: f64) -> RowFrame {
    RowFrame {
        min_y,
        height: 30.0,
    }
}

/// Without a bottom limit the label stays centered on the pointer.
#[test]
fn test_label_stays_centered_on_the_pointer_without_a_bottom_limit() {
    let (_, y) = label_origin_within((200.0, 90.0), (100.0, 60.0), 300.0, None);
    assert_eq!(y, 60.0);
}

/// Near the bottom of its container the label is raised so it ends at the
/// bottom edge; far enough from it the limit changes nothing.
#[test]
fn test_label_is_raised_above_the_bottom_edge() {
    let size = (100.0, 60.0);
    assert_eq!(
        label_origin_within((200.0, 90.0), size, 300.0, Some(100.0)),
        (200.0 - POINTER_GAP - 100.0, 40.0)
    );
    assert_eq!(
        label_origin_within((200.0, 30.0), size, 300.0, Some(100.0)).1,
        0.0
    );
}

/// A label taller than its container may rise above the container's top.
#[test]
fn test_tall_label_rises_above_the_container() {
    let (_, y) = label_origin_within((200.0, 20.0), (100.0, 160.0), 300.0, Some(80.0));
    assert_eq!(y, -80.0);
}

/// The row containing the pointer wins.
#[test]
fn test_nearest_row_contains_the_pointer() {
    let rows = [row(0.0), row(40.0), row(80.0)];
    assert_eq!(nearest_row_index(10.0, &rows), Some(0));
    assert_eq!(nearest_row_index(55.0, &rows), Some(1));
    assert_eq!(nearest_row_index(110.0, &rows), Some(2));
}

/// The gaps between rows, and the space above and below them, still pick a
/// row so the label does not flicker off between rows.
#[test]
fn test_nearest_row_covers_the_gaps_between_rows() {
    let rows = [row(20.0), row(60.0)];
    assert_eq!(nearest_row_index(52.0, &rows), Some(0));
    assert_eq!(nearest_row_index(58.0, &rows), Some(1));
    assert_eq!(nearest_row_index(0.0, &rows), Some(0));
    assert_eq!(nearest_row_index(200.0, &rows), Some(1));
}

/// No rows, no pick.
#[test]
fn test_nearest_row_is_none_without_rows() {
    assert_eq!(nearest_row_index(10.0, &[]), None);
}

fn one_day(tokens: TokensInput) -> DailyBudgetInput {
    DailyBudgetInput {
        days: vec![DayInput {
            date: chrono::NaiveDate::from_ymd_opt(2026, 7, 9).unwrap(),
            spent: 5.0,
            budget: 100.0 / 7.0,
            is_prior_window: false,
            is_over_budget: false,
            tokens: Some(tokens),
        }],
        accent: tokenmon_core::Srgb::rgb(0.1, 0.2, 0.3),
        noun: AllowanceNoun::Weekly,
        pace: None,
        footer_kind: FooterKind::Generic,
        show_live_footer: false,
        has_navigation: false,
        can_go_next: false,
        fill_threshold: 0.005,
    }
}

fn value_text(tokens: TokensInput) -> Option<String> {
    let ctx = tokenmon_ui::panels::common::PanelContext {
        now: chrono::DateTime::from_timestamp(1_783_900_800, 0).unwrap(),
        calendar: tokenmon_core::usage::Calendar::utc(),
    };
    let vm = build_daily_budget(&one_day(tokens), &ctx).unwrap();
    vm.bars[0].hover.tokens.as_ref().unwrap().cost_text.clone()
}

/// The day's value marks unpriced usage with a trailing `+`.
#[test]
fn test_daily_value_text_marks_unpriced_usage() {
    let mut t = TokensInput {
        total: 20,
        output: 20,
        cost_usd: 12.4,
        ..TokensInput::default()
    };
    assert_eq!(value_text(t.clone()).as_deref(), Some("≈ $12.40"));
    t.has_unpriced = true;
    assert_eq!(value_text(t).as_deref(), Some("≈ $12.40+"));
}

/// Without spend there is no value text.
#[test]
fn test_daily_value_text_is_hidden_without_spend() {
    let t = TokensInput {
        total: 10,
        output: 10,
        has_unpriced: true,
        ..TokensInput::default()
    };
    assert_eq!(value_text(t), None);
}
