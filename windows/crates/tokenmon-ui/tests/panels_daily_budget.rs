//! Daily budget card, hover label, week paging, stale caption, models card.
#![allow(missing_docs, clippy::unwrap_used, clippy::assert_is_empty)]

mod panels_common;

use panels_common::*;
use tokenmon_core::ProviderId;
use tokenmon_ui::panels::common::*;

/// Range label for one month and across months.
#[test]
fn test_range_label_same_month_and_cross_month() {
    assert_eq!(
        range_label(day(2026, 10, 13), day(2026, 10, 19)),
        "October 13 – 19"
    );
    assert_eq!(
        range_label(day(2026, 9, 29), day(2026, 10, 5)),
        "September 29 – Oct 5"
    );
}

/// Hover usage lines from the spec (DailyBudgetHoverTests.testUsageLineDescribesTheDay).
#[test]
fn test_usage_line_describes_the_day() {
    let mut d = week([0.0; 7]).remove(0);
    d.spent = 31.0;
    assert_eq!(
        usage_line(&d, AllowanceNoun::Weekly),
        "31.0% of weekly · 217% of daily allowance"
    );
    d.spent = 0.0;
    assert_eq!(
        usage_line(&d, AllowanceNoun::Weekly),
        "0% of the 14.3% daily allowance"
    );
    d.budget = 0.0;
    assert_eq!(usage_line(&d, AllowanceNoun::Weekly), "No budget");
    d.is_prior_window = true;
    d.spent = 12.0;
    assert_eq!(
        usage_line(&d, AllowanceNoun::Monthly),
        "12.0% of the monthly pool before the provider reset it early"
    );
}

/// Bars: fill height rule, future days, labels, prior-window opacity.
#[test]
fn test_bars_fill_labels_and_future_days() {
    let mut days = week([0.0, 5.0, 100.0, 0.001, 7.0, 0.0, 30.0]);
    days[3].is_prior_window = true;
    days[6].date = day(2026, 7, 16); // future relative to 07-15
    let vm = build_daily_budget(&daily(days), &ctx()).unwrap();
    assert_eq!(vm.title, "Daily Budget");
    assert_eq!(vm.allowance_text.as_deref(), Some("14.3%"));
    assert_eq!(vm.range_label, "July 9 – 16");
    assert_eq!(vm.bars.len(), 7);
    assert_eq!(vm.bars[0].fill_height, 0.0);
    // 5 / 14.29 = 35% -> 23.8px
    assert!((vm.bars[1].fill_height - 68.0 * 5.0 / (100.0 / 7.0)).abs() < 1e-9);
    assert_eq!(vm.bars[2].fill_height, 68.0, "capped at the stem height");
    assert_eq!(vm.bars[3].fill_height, 0.0, "below the 0.005 threshold");
    assert_eq!(vm.bars[3].fill_opacity, 0.45);
    assert_eq!(vm.bars[6].fill_height, 0.0, "future days draw no fill");
    assert_eq!(vm.bars[6].label_opacity, 0.5);
    assert!(vm.bars[6].hover.usage_line.is_none());
    assert_eq!(vm.bars[0].weekday, "Th");
    assert_eq!(vm.bars[0].value_label, "0%");
    assert_eq!(vm.bars[0].hover.title, "Thursday, Jul 9");
}

/// Tiny non-zero usage gets the 6px minimum.
#[test]
fn test_fill_has_six_pixel_minimum() {
    let vm = build_daily_budget(&daily(week([0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])), &ctx()).unwrap();
    assert_eq!(vm.bars[0].fill_height, 6.0);
}

/// Empty day lists hide the card.
#[test]
fn test_empty_days_hide_the_card() {
    assert!(build_daily_budget(&daily(vec![]), &ctx()).is_none());
}

/// Footer rules: pace caption, no-usage, over allowance, past weeks, Grok note.
#[test]
fn test_footer_rules() {
    let mut input = daily(week([0.0; 7]));
    assert_eq!(
        build_daily_budget(&input, &ctx())
            .unwrap()
            .footer
            .as_deref(),
        Some("No usage yet · 14.3%/day")
    );
    input.pace_caption = Some("On pace for today".to_owned());
    assert_eq!(
        build_daily_budget(&input, &ctx())
            .unwrap()
            .footer
            .as_deref(),
        Some("On pace for today")
    );
    input.show_live_footer = false;
    assert!(build_daily_budget(&input, &ctx()).unwrap().footer.is_none());

    let mut over = daily(week([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 20.0]));
    over.days[6].is_over_budget = true;
    assert_eq!(
        build_daily_budget(&over, &ctx()).unwrap().footer.as_deref(),
        Some("Over today's allowance")
    );
    over.days[6].is_over_budget = false;
    assert!(build_daily_budget(&over, &ctx()).unwrap().footer.is_none());

    let mut grok = daily(week([1.0; 7]));
    grok.footer_kind = FooterKind::GrokWeek {
        show_estimate_note: true,
    };
    assert_eq!(
        build_daily_budget(&grok, &ctx()).unwrap().footer.as_deref(),
        Some("Bars update between polls · week totals above")
    );
    grok.footer_kind = FooterKind::GrokWeek {
        show_estimate_note: false,
    };
    assert!(build_daily_budget(&grok, &ctx()).unwrap().footer.is_none());
}

/// Token block: totals, cost, grid rows and the 3-model cap.
#[test]
fn test_hover_token_block() {
    let mut days = week([1.0; 7]);
    days[0].tokens = Some(TokensInput {
        total: 1_234_567,
        input: 1_000,
        output: 2_000_000,
        cache_read: 999_500,
        cache_write: 5,
        cost_usd: 12.5,
        has_unpriced: true,
        models: vec![
            ("A".into(), 1_000_000),
            ("B".into(), 2_000),
            ("C".into(), 3),
            ("D".into(), 1),
        ],
    });
    days[1].tokens = Some(TokensInput::default());
    let vm = build_daily_budget(&daily(days), &ctx()).unwrap();
    let t = vm.bars[0].hover.tokens.as_ref().unwrap();
    assert_eq!(t.total_text, "1.2M");
    assert_eq!(t.cost_text.as_deref(), Some("≈ $12.50+"));
    assert_eq!(t.rows[0], ("Input".to_owned(), "1K".to_owned()));
    assert_eq!(t.rows[1].1, "2.0M");
    assert_eq!(t.rows[2], ("Cache read".to_owned(), "1.0M".to_owned()));
    assert_eq!(t.rows[3], ("Cache write".to_owned(), "5".to_owned()));
    assert_eq!(t.models.len(), 3);
    assert!(
        vm.bars[1].hover.tokens.is_none(),
        "zero total hides the block"
    );
}

/// `WeekOffset` paging (DailyBudgetWeekOffsetTests).
#[test]
fn test_week_offset_never_moves_past_the_current_week() {
    assert_eq!(week_previous(0), -1);
    assert_eq!(week_previous(-2), -3);
    assert_eq!(week_next(0), 0);
    assert_eq!(week_next(-1), 0);
    assert_eq!(week_next(-4), -3);
    assert!(!week_can_go_next(0));
    assert!(week_can_go_next(-1));
}

/// Stale caption includes the age only when a refresh time is known.
#[test]
fn test_stale_caption() {
    let mut s = status();
    assert!(StaleVm::from_status(&s, &ctx()).is_none());
    s.last_error = Some("HTTP 500".into());
    let v = StaleVm::from_status(&s, &ctx()).unwrap();
    assert_eq!((v.message.as_str(), v.age_line), ("HTTP 500", None));
    s.last_refreshed_at = Some(at(2026, 7, 15, 11, 55));
    let v = StaleVm::from_status(&s, &ctx()).unwrap();
    assert_eq!(v.age_line.as_deref(), Some("Showing data from 5 min ago."));
}

/// Sign-in/out labels and tooltips.
#[test]
fn test_sign_in_and_out_strings() {
    assert_eq!(
        SignInAction::to_provider(ProviderId::ChatGpt).label,
        "Sign In to ChatGPT…"
    );
    assert_eq!(
        SignInAction::again(ProviderId::Cursor).label,
        "Sign In Again…"
    );
    assert_eq!(
        SignOutVm::new(ProviderId::Cursor).confirm_text,
        "Sign out of Cursor?"
    );
    assert_eq!(SignOutVm::TOOLTIP, "Sign out");
    assert_eq!(SignOutVm::CONFIRM_TOOLTIP, "Confirm sign out");
    assert_eq!(SignOutVm::CANCEL_TOOLTIP, "Cancel");
}

/// Models card shows 3 rows until `View all`, and hides rows while collapsed.
#[test]
fn test_models_card_truncation_and_collapse() {
    let rows: Vec<ModelRowVm> = (0..5)
        .map(|i| ModelRowVm {
            name: format!("m{i}"),
            cost_text: "$1.00".into(),
            tag: None,
            share_fraction: 0.2,
            color: tokenmon_core::Srgb::rgb(0.0, 0.0, 0.0),
        })
        .collect();
    let c = ModelsCardVm::new("T", true, true, rows.clone(), false);
    assert_eq!((c.rows.len(), c.toggle_label), (3, Some("View all")));
    let c = ModelsCardVm::new("T", true, true, rows.clone(), true);
    assert_eq!((c.rows.len(), c.toggle_label), (5, Some("Show less")));
    let c = ModelsCardVm::new("T", true, false, rows.clone(), true);
    assert!(c.rows.is_empty() && !c.expanded);
    let c = ModelsCardVm::new("T", false, false, rows[..2].to_vec(), false);
    assert!(c.expanded && c.toggle_label.is_none() && c.rows.len() == 2);
}
