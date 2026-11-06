//! Port of Swift `DailyTokenUsageTests` (core parts) and `UsagePool` basics.

#![allow(
    clippy::unwrap_used,
    clippy::needless_pass_by_value,
    clippy::useless_vec,
    clippy::unreadable_literal,
    clippy::float_cmp
)]

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::usage::{
    Calendar, DailyBudgetDay, DailyTokenUsage, DailyTokenUsageTally, DayKey, ModelTokens,
    TokenCounts, UsagePool, attaching_tokens_map,
};

fn t(s: &str) -> DateTime<Utc> {
    tokenmon_core::util::parse_flexible(s).expect("valid fixture time")
}

fn input(n: i64) -> TokenCounts {
    TokenCounts {
        input: n,
        ..Default::default()
    }
}

/// Models merge by name and sort by tokens descending.
#[test]
fn test_add_merges_models_and_sorts_by_tokens() {
    let mut u = DailyTokenUsage::default();
    u.add(
        "A",
        TokenCounts {
            input: 10,
            output: 5,
            ..Default::default()
        },
        Some(1.0),
    );
    u.add(
        "B",
        TokenCounts {
            cache_read: 100,
            ..Default::default()
        },
        Some(2.0),
    );
    u.add(
        "A",
        TokenCounts {
            cache_write: 1,
            ..Default::default()
        },
        Some(0.5),
    );
    assert_eq!(u.total_tokens(), 116);
    assert_eq!(u.cost_usd, Some(3.5));
    assert!(!u.has_unpriced);
    assert_eq!(
        u.models,
        vec![
            ModelTokens {
                name: "B".into(),
                tokens: 100
            },
            ModelTokens {
                name: "A".into(),
                tokens: 16
            }
        ]
    );
}

/// Unpriced usage is flagged; an empty model name counts toward totals only.
#[test]
fn test_unpriced_usage_is_flagged() {
    let mut u = DailyTokenUsage::default();
    u.add(
        "X",
        TokenCounts {
            output: 3,
            ..Default::default()
        },
        None,
    );
    assert_eq!(u.cost_usd, None);
    assert!(u.has_unpriced);
    u.add("", input(2), Some(0.25));
    assert_eq!(u.cost_usd, Some(0.25));
    assert_eq!(
        u.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        ["X"]
    );
}

/// Equal token counts sort by name ascending.
#[test]
fn test_model_ties_sort_by_name() {
    let mut u = DailyTokenUsage::default();
    for name in ["b", "a"] {
        u.add(name, input(5), Some(0.0));
    }
    assert_eq!(u.models[0].name, "a");
}

/// Tally buckets by local day and attaches to the matching bar only.
#[test]
fn test_tally_groups_by_local_day_and_attaches_to_bars() {
    let utc = Calendar::utc();
    let mut tally = DailyTokenUsageTally::new(utc);
    tally.add_at(t("2026-11-03T23:59:00Z"), "A", input(1), None);
    tally.add_at(t("2026-11-04T00:01:00Z"), "A", input(2), None);
    assert_eq!(
        tally.days()[&DayKey::from_raw("2026-11-03")].input_tokens,
        1
    );
    assert_eq!(
        tally.days()[&DayKey::from_raw("2026-11-04")].input_tokens,
        2
    );

    let by_day: HashMap<_, _> = tally.days().clone().into_iter().collect();
    let bars = vec![
        DailyBudgetDay::new(t("2026-11-04T00:00:00Z"), 1.0, 2.0),
        DailyBudgetDay::new(t("2026-11-05T00:00:00Z"), 0.0, 2.0),
    ];
    let bars = attaching_tokens_map(bars, &by_day, &utc);
    assert_eq!(bars[0].tokens.as_ref().map(|u| u.input_tokens), Some(2));
    assert!(bars[1].tokens.is_none());
    let unchanged = attaching_tokens_map(bars.clone(), &HashMap::new(), &utc);
    assert_eq!(unchanged, bars);
}

/// The tally uses the injected offset, not the host zone.
#[test]
fn test_tally_respects_injected_offset() {
    let tokyo = Calendar::fixed(chrono::FixedOffset::east_opt(9 * 3600).unwrap());
    let mut tally = DailyTokenUsageTally::new(tokyo);
    tally.add_at(
        Utc.with_ymd_and_hms(2026, 11, 3, 16, 0, 0).unwrap(),
        "A",
        TokenCounts::default(),
        None,
    );
    assert!(tally.days().contains_key(&DayKey::from_raw("2026-11-04")));
}

/// Cadence collapsing and section titles.
#[test]
fn test_usage_pool_combining_and_titles() {
    use UsagePool::{Hourly, Mixed, Monthly, Weekly};
    assert_eq!(UsagePool::combining(&[Weekly, Weekly]), Weekly);
    assert_eq!(UsagePool::combining(&[Monthly]), Monthly);
    assert_eq!(UsagePool::combining(&[Hourly, Weekly]), Mixed);
    assert_eq!(UsagePool::combining(&[]), Mixed);
    assert_eq!(Weekly.section_title(), "Usage Pool Weekly");
    for p in [Hourly, Weekly, Monthly, Mixed] {
        assert_eq!(p.to_string().parse::<UsagePool>(), Ok(p));
    }
    assert!("nope".parse::<UsagePool>().is_err());
}
