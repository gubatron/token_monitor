//! Shared fixtures for the panel view-model tests.
#![allow(dead_code, missing_docs, clippy::unwrap_used)]

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use tokenmon_core::Srgb;
use tokenmon_ui::panels::common::{
    AllowanceNoun, DailyBudgetInput, DayInput, FooterKind, PanelContext, ProviderStatus,
};

/// 2026-07-15 12:00 UTC, zone UTC.
pub fn ctx() -> PanelContext {
    PanelContext {
        now: Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap(),
        tz: FixedOffset::east_opt(0).unwrap(),
    }
}

pub fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
}

pub fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

pub fn status() -> ProviderStatus {
    ProviderStatus::default()
}

pub fn signed_out() -> ProviderStatus {
    ProviderStatus {
        needs_sign_in: true,
        ..ProviderStatus::default()
    }
}

pub fn daily(days: Vec<DayInput>) -> DailyBudgetInput {
    DailyBudgetInput {
        days,
        accent: Srgb::rgb(0.1, 0.2, 0.3),
        noun: AllowanceNoun::Weekly,
        pace_caption: None,
        footer_kind: FooterKind::Generic,
        show_live_footer: true,
        has_navigation: true,
        can_go_next: false,
        fill_threshold: 0.005,
    }
}

/// Seven days 2026-07-09..=07-15 (Thu..Wed), each `spent`, budget 100/7.
pub fn week(spent: [f64; 7]) -> Vec<DayInput> {
    (0..7)
        .map(|i| DayInput {
            date: day(2026, 7, 9 + i as u32),
            spent: spent[i],
            budget: 100.0 / 7.0,
            is_prior_window: false,
            is_over_budget: false,
            tokens: None,
        })
        .collect()
}
