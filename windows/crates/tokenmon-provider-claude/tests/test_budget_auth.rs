//! Daily-bar data, hover tokens and sign-in rules
//! (`ClaudeUsagePollerTests` pure parts, `SignInBrowserNavigationTests`,
//! `ProviderCookieAllowlistTests::testClaudePolicyKeepsSessionKeyAndOrg`).

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use tokenmon_core::usage::{Calendar, DailyBudgetDay, DayKey, WeekOffset};
use tokenmon_provider_claude::auth::{claude_auth_config, is_auth_host, is_return_page};
use tokenmon_provider_claude::budget::{SpentByDay, attach_tokens, build_daily_budget_days};
use tokenmon_provider_claude::{DailyTokens, ModelTokens};

fn t(m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, m, d, h, 0, 0).unwrap()
}

/// Reset 08-27 11:00 and now 08-25 15:00 give 7 bars, Aug 20 to Aug 26.
#[test]
fn test_build_daily_budget_days_anchors_first_bar_to_period_start() {
    let days = build_daily_budget_days(
        &SpentByDay::new(),
        Some(t(8, 27, 11)),
        None,
        None,
        WeekOffset::CURRENT,
        t(8, 25, 15),
        &Calendar::utc(),
    );
    assert_eq!(days.len(), 7);
    assert_eq!(days[0].date, t(8, 20, 0));
    assert_eq!(days[6].date, t(8, 26, 0));
    assert!((days[0].budget_usd - 100.0 / 7.0).abs() < 1e-9);
}

/// Without a reset time there are no bars.
#[test]
fn test_build_daily_budget_days_empty_without_reset_time() {
    let days = build_daily_budget_days(
        &SpentByDay::new(),
        None,
        None,
        None,
        WeekOffset::CURRENT,
        t(8, 25, 15),
        &Calendar::utc(),
    );
    assert!(days.is_empty());
}

/// An earlier week shows its own spend without a prior-window lead-in.
#[test]
fn test_build_daily_budget_days_week_offset_shows_prior_period() {
    let mut spent = SpentByDay::new();
    spent.insert(t(8, 14, 0), 6.0);
    let days = build_daily_budget_days(
        &spent,
        Some(t(8, 27, 11)),
        None,
        None,
        WeekOffset::CURRENT.previous(),
        t(8, 25, 15),
        &Calendar::utc(),
    );
    assert_eq!(days[0].date, t(8, 13, 0));
    assert_eq!(days[1].spent_usd, 6.0);
    assert!(days.iter().all(|d| !d.is_prior_window));
}
/// Each bar gets its local day's tokens; other days keep none and spend is untouched.
#[test]
fn test_attach_tokens_adds_each_local_days_tokens() {
    let denver = Calendar::fixed(FixedOffset::west_opt(7 * 3600).unwrap());
    let day1 = Utc.with_ymd_and_hms(2026, 11, 2, 7, 0, 0).unwrap();
    let day2 = Utc.with_ymd_and_hms(2026, 11, 3, 7, 0, 0).unwrap();
    let tok = |i, o, cr, w5, w1| ModelTokens {
        input_tokens: i,
        output_tokens: o,
        cache_read_tokens: cr,
        cache_write_5m_tokens: w5,
        cache_write_1h_tokens: w1,
        response_count: 1,
    };
    let mut models = BTreeMap::new();
    models.insert("claude-opus-5-5".to_owned(), tok(10, 20, 1_000_000, 30, 40));
    models.insert("claude-sonnet-5".to_owned(), tok(5, 5, 0, 0, 0));
    models.insert("mystery".to_owned(), tok(0, 1, 0, 0, 0));
    let local: DailyTokens = BTreeMap::from([(DayKey::from_raw("2026-11-02"), models)]);
    let bars = vec![
        DailyBudgetDay::new(day1, 5.0, 14.0),
        DailyBudgetDay::new(day2, 1.0, 14.0),
    ];
    let out = attach_tokens(bars.clone(), &local, &denver);
    let d = out[0].tokens.clone().unwrap();
    assert_eq!(
        (
            d.input_tokens,
            d.output_tokens,
            d.cache_read_tokens,
            d.cache_write_tokens
        ),
        (15, 26, 1_000_000, 70)
    );
    assert_eq!(d.total_tokens(), 1_000_111);
    assert_eq!(d.models.len(), 3);
    assert_eq!(d.models[0].name, "Opus 5.5");
    assert!(d.cost_usd.unwrap() > 0.0 && d.has_unpriced);
    assert!(out[1].tokens.is_none());
    assert_eq!(out[0].spent_usd, 5.0);
    assert_eq!(
        attach_tokens(bars.clone(), &DailyTokens::new(), &denver),
        bars
    );
}

/// The ledger buckets by UTC; `Local` and `Fixed` exist for the bars.
#[test]
fn test_ledger_days_are_utc() {
    let late = Utc.with_ymd_and_hms(2026, 10, 10, 23, 30, 0).unwrap();
    assert_eq!(
        DayKey::for_instant(late, &Calendar::utc()).as_str(),
        "2026-10-10"
    );
    assert_eq!(
        DayKey::for_instant(late, &Calendar::fixed(FixedOffset::east_opt(3600).unwrap())).as_str(),
        "2026-10-11"
    );
}

/// The Claude cookie policy keeps `sessionKey` and `lastActiveOrg` only, case-insensitively.
#[test]
fn test_claude_policy_keeps_session_key_and_org() {
    let c = claude_auth_config();
    assert!(c.is_essential("sessionKey") && c.is_essential("LASTACTIVEORG"));
    assert!(!c.is_essential("__cf_bm") && !c.is_essential("ajs_user_id"));
    assert_eq!(c.store_filename_prefix, "claude_auth_");
    assert_eq!(c.account_identity_cookie.as_deref(), Some("lastactiveorg"));
    assert_eq!(c.max_attempts, 4);
    assert_eq!(c.hosts, ["claude.ai", "www.claude.ai", "api.claude.ai"]);
}

/// Clerk and auth paths never count as the return page.
#[test]
fn test_claude_return_page_excludes_clerk_and_auth_paths() {
    assert!(is_auth_host("clerk.claude.ai", "/sign-in"));
    // Exact or dot-suffix host matching, never substring (issue #41).
    assert!(!is_auth_host("github.com.evil.example", "/"));
    assert!(!is_auth_host("evilgithub.com", "/"));
    assert!(!is_auth_host("accounts.google.com.evil.example", "/"));
    assert!(!is_auth_host("clerk.claude.ai.evil.example", "/"));
    assert!(!is_auth_host("evil.example", "/login"));
    assert!(is_auth_host("github.com", "/login/oauth"));
    assert!(is_auth_host("www.claude.ai", "/login"));
    assert!(!is_return_page("clerk.claude.ai", "/sign-in"));
    assert!(is_return_page("claude.ai", "/new"));
    assert!(is_return_page("www.claude.ai", "/chat/abc"));
    assert!(!is_return_page("claude.ai", "/login"));
    assert!(!is_return_page("claude.ai", "/signin"));
    assert!(!is_return_page("claude.ai", "/auth/x"));
    assert!(!is_return_page("auth.claude.ai", "/new"));
}
