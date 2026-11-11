//! Month-over-month change of a model's spend or tokens, and its wording
//! (Swift `MonthlyUsageChangeTests`), plus the hover label's shared value and
//! breakdown helpers (Swift `HoverLabelTests`, "Rows" and "Daily label value").

#![allow(clippy::unwrap_used)]

use tokenmon_core::usage::{
    ChangeUnit, DailyTokenUsage, HoverLabelRow, MonthlyUsageChange, TokenCounts, api_value_text,
};

fn usd(previous: Option<f64>, current: f64) -> MonthlyUsageChange {
    MonthlyUsageChange::between(previous, current, ChangeUnit::Usd)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.0001
}

// ---- Classification -------------------------------------------------------

/// An increase carries the amount and the percent.
#[test]
fn test_increase_carries_the_amount_and_percent() {
    let MonthlyUsageChange::Increase { amount, percent } = usd(Some(100.0), 118.0) else {
        panic!("expected an increase");
    };
    assert!(close(amount, 18.0));
    assert!(close(percent.unwrap(), 18.0));
}

/// A decrease carries a positive amount and percent.
#[test]
fn test_decrease_carries_positive_amount_and_percent() {
    let MonthlyUsageChange::Decrease { amount, percent } = usd(Some(200.0), 150.0) else {
        panic!("expected a decrease");
    };
    assert!(close(amount, 50.0));
    assert!(close(percent.unwrap(), 25.0));
}

/// No usage last month is new, whatever this month is.
#[test]
fn test_no_usage_last_month_is_new() {
    assert_eq!(usd(None, 3.91), MonthlyUsageChange::New);
    assert_eq!(usd(None, 0.0), MonthlyUsageChange::New);
}

/// Differences that round away in the unit count as no change.
#[test]
fn test_difference_below_the_resolution_is_unchanged() {
    assert_eq!(usd(Some(12.40), 12.404), MonthlyUsageChange::Unchanged);
    assert_eq!(usd(Some(0.0), 0.0), MonthlyUsageChange::Unchanged);
    let tokens = MonthlyUsageChange::between(Some(1_000.0), 1_000.0, ChangeUnit::Tokens);
    assert_eq!(tokens, MonthlyUsageChange::Unchanged);
    assert_ne!(usd(Some(12.40), 12.41), MonthlyUsageChange::Unchanged);
}

/// Last month priced at $0.00 gives the amount but no percent.
#[test]
fn test_zero_spend_last_month_has_no_percent() {
    let MonthlyUsageChange::Increase { amount, percent } = usd(Some(0.001), 3.91) else {
        panic!("expected an increase");
    };
    assert!(close(amount, 3.909));
    assert_eq!(percent, None);
    assert_eq!(
        usd(Some(0.0), 3.91),
        MonthlyUsageChange::Increase {
            amount: 3.91,
            percent: None
        }
    );
}

/// Unit resolutions and names round-trip, and an unknown name is rejected.
#[test]
fn test_unit_resolution_and_raw_values() {
    assert!(close(ChangeUnit::Usd.resolution(), 0.005));
    assert!(close(ChangeUnit::Tokens.resolution(), 0.5));
    for unit in ChangeUnit::ALL {
        assert_eq!(unit.as_str().parse::<ChangeUnit>().unwrap(), unit);
    }
    assert!("euros".parse::<ChangeUnit>().is_err());
}

// ---- Wording --------------------------------------------------------------

/// "↑ 14% · +$212.40 vs October".
#[test]
fn test_increase_line() {
    let change = usd(Some(1_500.05), 1_712.45);
    assert_eq!(
        change.summary("October", ChangeUnit::Usd),
        "↑ 14% · +$212.40 vs October"
    );
}

/// A decrease uses a true minus sign.
#[test]
fn test_decrease_line_uses_a_minus_sign() {
    let change = usd(Some(880.0), 749.51);
    assert_eq!(
        change.summary("October", ChangeUnit::Usd),
        "↓ 15% · \u{2212}$130.49 vs October"
    );
}

/// A rise from zero spend gives only the amount.
#[test]
fn test_increase_from_zero_spend_gives_only_the_amount() {
    assert_eq!(
        usd(Some(0.0), 3.91).summary("October", ChangeUnit::Usd),
        "↑ $3.91 vs October"
    );
}

/// A drop to zero is a full decrease.
#[test]
fn test_drop_to_zero_spend_is_a_full_decrease() {
    assert_eq!(
        usd(Some(2.50), 0.0).summary("October", ChangeUnit::Usd),
        "↓ 100% · \u{2212}$2.50 vs October"
    );
}

/// The two fixed lines.
#[test]
fn test_new_and_unchanged_lines() {
    assert_eq!(
        MonthlyUsageChange::New.summary("October", ChangeUnit::Usd),
        "New this month"
    );
    assert_eq!(
        MonthlyUsageChange::Unchanged.summary("October", ChangeUnit::Usd),
        "Same as October"
    );
}

/// Token changes print token counts.
#[test]
fn test_token_lines() {
    let t = ChangeUnit::Tokens;
    let rise = MonthlyUsageChange::between(Some(1_000_000.0), 1_200_000.0, t);
    assert_eq!(
        rise.summary("October", t),
        "↑ 20% · +200K tokens vs October"
    );
    let drop = MonthlyUsageChange::between(Some(4_000_000.0), 1_000_000.0, t);
    assert_eq!(
        drop.summary("October", t),
        "↓ 75% · \u{2212}3.0M tokens vs October"
    );
}

/// The spoken lines use words instead of arrows.
#[test]
fn test_spoken_lines_use_words_instead_of_arrows() {
    let u = ChangeUnit::Usd;
    assert_eq!(
        usd(Some(1_500.05), 1_712.45).spoken_summary("October", u),
        "Up 14%, $212.40 more than October"
    );
    assert_eq!(
        usd(Some(880.0), 749.51).spoken_summary("October", u),
        "Down 15%, $130.49 less than October"
    );
    assert_eq!(
        usd(Some(0.0), 3.91).spoken_summary("October", u),
        "$3.91 more than October"
    );
    let to_zero = MonthlyUsageChange::Decrease {
        amount: 3.91,
        percent: None,
    };
    assert_eq!(
        to_zero.spoken_summary("October", u),
        "$3.91 less than October"
    );
    assert_eq!(
        MonthlyUsageChange::New.spoken_summary("October", u),
        "New this month"
    );
    assert_eq!(
        MonthlyUsageChange::Unchanged.spoken_summary("October", u),
        "Same as October"
    );
}

// ---- Percent rounding -----------------------------------------------------

/// Halves round away from zero and the sign is dropped.
#[test]
fn test_percent_rounds_half_away_from_zero() {
    assert_eq!(MonthlyUsageChange::percent_text(14.5), "15%");
    assert_eq!(MonthlyUsageChange::percent_text(14.49), "14%");
    assert_eq!(MonthlyUsageChange::percent_text(-25.0), "25%");
    assert_eq!(MonthlyUsageChange::percent_text(1_250.0), "1250%");
}

/// A tiny change never reads 0% and a near-total drop never reads 100%.
#[test]
fn test_percent_does_not_overstate_the_extremes() {
    assert_eq!(MonthlyUsageChange::percent_text(0.3), "<1%");
    assert_eq!(MonthlyUsageChange::percent_text(0.5), "1%");
    assert_eq!(MonthlyUsageChange::percent_text(99.7), ">99%");
    assert_eq!(MonthlyUsageChange::percent_text(100.0), "100%");
    assert_eq!(MonthlyUsageChange::percent_text(100.2), "100%");
}

// ---- Hover label helpers --------------------------------------------------

fn counts(output: i64) -> TokenCounts {
    TokenCounts {
        output,
        ..TokenCounts::default()
    }
}

/// The breakdown lists input, output and both cache counts in order.
#[test]
fn test_breakdown_lists_input_output_and_both_cache_counts() {
    let rows = HoverLabelRow::breakdown(1_200, 3_400_000, 999, 0);
    let pairs: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.label.as_str(), r.value.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("Input", "1K"),
            ("Output", "3.4M"),
            ("Cache read", "999"),
            ("Cache write", "0")
        ]
    );
}

/// The daily value marks unpriced usage with a trailing `+`.
#[test]
fn test_daily_value_text_marks_unpriced_usage() {
    let mut day = DailyTokenUsage::default();
    day.add("Opus 5.5", counts(10), Some(12.4));
    assert_eq!(day.value_text().as_deref(), Some("≈ $12.40"));
    day.add("Mystery", counts(10), None);
    assert_eq!(day.value_text().as_deref(), Some("≈ $12.40+"));
}

/// Without spend there is no value text.
#[test]
fn test_daily_value_text_is_hidden_without_spend() {
    let mut day = DailyTokenUsage::default();
    day.add("Mystery", counts(10), None);
    assert_eq!(day.value_text(), None);
    assert_eq!(DailyTokenUsage::default().value_text(), None);
    assert_eq!(api_value_text(Some(0.0), false), None);
}
