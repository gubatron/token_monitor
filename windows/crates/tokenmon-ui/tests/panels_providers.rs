//! Provider panel view models: every state and exact string.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::many_single_char_names
)]

mod panels_common;

use panels_common::*;
use tokenmon_core::{ProviderId, Srgb};
use tokenmon_ui::panels::claude_chatgpt::*;
use tokenmon_ui::panels::common::*;
use tokenmon_ui::panels::grok::{self, *};
use tokenmon_ui::panels::grokbot_cursor::*;
use tokenmon_ui::panels::opencode_openrouter::*;

fn product(name: &str, pct: f64) -> ProductInput {
    ProductInput {
        short_name: name.into(),
        percent: pct,
        color: Srgb::rgb(0.5, 0.5, 0.5),
    }
}

fn grok_snap() -> GrokSnapshotInput {
    GrokSnapshotInput {
        used_percent: 36.4,
        resets_at: Some(at(2026, 7, 16, 20, 25)),
        pool_products: vec![product("Chat", 23.0), product("Build", 13.0)],
        category_products: vec![
            product("Chat", 23.0),
            product("Build", 12.6),
            product("Imagine", 0.0),
            product("Voice", 0.4),
        ],
        extra_credits_balance: 4.1,
        week: Some(daily(week([1.0; 7]))),
    }
}

fn grok_input(
    snapshot: Option<GrokSnapshotInput>,
    status: ProviderStatus,
    signed_in: bool,
) -> GrokInput {
    GrokInput {
        status,
        is_signed_in: signed_in,
        snapshot,
    }
}

// ---------------------------------- Grok -----------------------------------

/// Grok state priority: expired, usage, waiting, signed-out.
#[test]
fn test_grok_states() {
    let mut s = signed_out();
    s.has_auth_error = true;
    let p = grok::build(&grok_input(Some(grok_snap()), s.clone(), true), &ctx());
    assert_eq!(p.kind, PanelKind::SessionExpired);
    assert_eq!(p.header.title, "SuperGrok");
    assert_eq!(
        p.message.as_deref(),
        Some("Session expired. Sign in again to load usage.")
    );
    assert_eq!(p.sign_in.as_ref().unwrap().label, "Sign In Again…");
    s.last_error = Some("boom".into());
    assert_eq!(
        grok::build(&grok_input(None, s, true), &ctx())
            .message
            .as_deref(),
        Some("boom")
    );

    let p = grok::build(&grok_input(None, status(), false), &ctx());
    assert_eq!(p.kind, PanelKind::SignedOut);
    assert_eq!(p.message.as_deref(), Some("Sign in to load your usage."));
    assert_eq!(p.sign_in.unwrap().label, "Sign In…");

    let p = grok::build(&grok_input(None, status(), true), &ctx());
    assert_eq!(
        (p.kind, p.message.as_deref()),
        (
            PanelKind::Waiting,
            Some("Signed in — waiting for usage data.")
        )
    );
    assert!(p.sign_in.is_none());
    let mut s = status();
    s.is_refreshing = true;
    assert_eq!(
        grok::build(&grok_input(None, s, true), &ctx())
            .message
            .as_deref(),
        Some("Refreshing…")
    );
    let p = grok::build(&grok_input(None, signed_out(), true), &ctx());
    assert_eq!(p.sign_in.unwrap().label, "Sign In Again…");
}

/// Grok usage body strings.
#[test]
fn test_grok_usage_body() {
    let p = grok::build(&grok_input(Some(grok_snap()), status(), true), &ctx());
    let u = p.usage.unwrap();
    assert_eq!(u.pool_title, "Usage Pool Weekly");
    assert_eq!(u.pool_used_text, "36% Used");
    assert_eq!(
        u.reset_caption.as_deref(),
        Some("Resets Thu 16 July 8:25pm")
    );
    let chips: Vec<String> = u
        .categories
        .unwrap()
        .iter()
        .map(|c| format!("{} {}", c.name, c.percent_text))
        .collect();
    assert_eq!(chips, ["Chat 23%", "Build 13%", "Imagine 0%", "Voice 0%"]);
    assert_eq!(u.extra_credits.as_deref(), Some("$4.10"));
    assert!(u.daily.is_some());
    assert_eq!(u.footer.sign_out.unwrap().confirm_text, "Sign out of Grok?");
}

/// Hidden cards: no categories, no extra credits, no week.
#[test]
fn test_grok_optional_cards_hidden() {
    let mut snap = grok_snap();
    snap.category_products.clear();
    snap.extra_credits_balance = 0.0;
    snap.week = None;
    snap.resets_at = None;
    let u = grok::build(&grok_input(Some(snap), status(), true), &ctx())
        .usage
        .unwrap();
    assert!(
        u.categories.is_none()
            && u.extra_credits.is_none()
            && u.daily.is_none()
            && u.reset_caption.is_none()
    );
}

/// Stale caption appears under the usage body.
#[test]
fn test_grok_stale_footer() {
    let mut s = status();
    s.last_error = Some("net down".into());
    s.last_refreshed_at = Some(at(2026, 7, 15, 9, 0));
    let u = grok::build(&grok_input(Some(grok_snap()), s, true), &ctx())
        .usage
        .unwrap();
    let stale = u.footer.stale.unwrap();
    assert_eq!(
        stale.age_line.as_deref(),
        Some("Showing data from 3 hr ago.")
    );
}

/// SegmentedUsageBarTests plus layout of the pool bar.
#[test]
fn test_segment_widths_and_layout() {
    assert_eq!(
        segment_widths(&[50.0, 25.0, 25.0], 100.0, 3.0),
        vec![50.0, 25.0, 25.0]
    );
    let w = segment_widths(&[0.1, 0.1, 0.1], 5.0, 3.0);
    assert!((w.iter().sum::<f64>() - 5.0).abs() < 1e-4);
    assert_eq!(segment_widths(&[50.0, 50.0], 0.0, 3.0), vec![3.0, 3.0]);

    let segs = pool_segments(
        &[
            product("Chat", 23.0),
            product("Build", 13.0),
            product("Tiny", 0.04),
        ],
        372.0,
    );
    assert_eq!(segs.len(), 3, "two visible + grey remainder");
    assert!(segs[2].color.is_none());
    assert_eq!(segs[0].x, 0.0);
    assert!((segs[1].x - (segs[0].width + 2.0)).abs() < 1e-9);
    let last = segs.last().unwrap();
    assert!((last.x + last.width - 372.0).abs() < 1e-6);
    assert!(
        pool_segments(&[product("A", 100.0)], 372.0).len() == 1,
        "no remainder at 100%"
    );
    assert!(pool_segments(&[], 372.0).is_empty() || pool_segments(&[], 372.0).len() == 1);
}

// --------------------------------- Grokbot ---------------------------------

fn grokbot_snap(allowance: bool) -> GrokbotSnapshotInput {
    GrokbotSnapshotInput {
        pool_name: "Weekly".into(),
        used_percent: 41.6,
        entitlement_caption: "Included with SuperGrok".into(),
        has_included_allowance: allowance,
        resets_at: Some(at(2026, 7, 20, 0, 0)),
        daily: Some(daily(week([2.0; 7]))),
    }
}

/// Grokbot signed-out text and Cursor sign-in target.
#[test]
fn test_grokbot_signed_out_opens_cursor() {
    let p = build_grokbot(
        &GrokbotInput {
            status: signed_out(),
            snapshot: None,
        },
        &ctx(),
    );
    assert_eq!(p.kind, PanelKind::SignedOut);
    assert_eq!(
        p.message.as_deref(),
        Some(
            "Sign in to Cursor to load your weekly Grokbot allowance. Grokbot is billed through Cursor even when a SuperGrok plan pays for it."
        )
    );
    let a = p.sign_in.unwrap();
    assert_eq!(
        (a.label.as_str(), a.provider),
        ("Sign In to Grokbot…", ProviderId::Cursor)
    );
}

/// Grokbot snapshot with and without an included allowance.
#[test]
fn test_grokbot_usage() {
    let u = build_grokbot(
        &GrokbotInput {
            status: status(),
            snapshot: Some(grokbot_snap(true)),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert_eq!(u.pool_title, "Usage Pool Weekly");
    assert_eq!(u.pool_used_text, "42% Used");
    let t = u.track.unwrap();
    assert!(!t.show_label);
    assert_eq!(t.caption.as_deref(), Some("Resets Mon 20 July 12:00am"));
    assert!(u.daily.is_some() && u.no_allowance_text.is_none());
    assert_eq!(
        u.footer.sign_out.unwrap().confirm_text,
        "Sign out of Cursor?"
    );

    let u = build_grokbot(
        &GrokbotInput {
            status: signed_out(),
            snapshot: Some(grokbot_snap(false)),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert!(u.track.is_none() && u.daily.is_none());
    assert_eq!(
        u.no_allowance_text,
        Some("This plan has no included Bot allowance — usage bills on demand.")
    );
    assert_eq!(u.footer.sign_in_again.unwrap().label, "Sign In Again…");
}

/// Grokbot with no snapshot, signed in.
#[test]
fn test_grokbot_waiting() {
    let p = build_grokbot(
        &GrokbotInput {
            status: status(),
            snapshot: None,
        },
        &ctx(),
    );
    assert_eq!(
        (p.kind, p.message.as_deref()),
        (PanelKind::Waiting, Some("No usage data yet."))
    );
}

// ---------------------------------- Cursor ---------------------------------

fn cursor_snap() -> CursorSnapshotInput {
    CursorSnapshotInput {
        plan_name: "Cursor Ultra".into(),
        pools: vec![
            CursorPoolInput {
                label: "Total".into(),
                used_percent: 55.2,
                resets_at: None,
            },
            CursorPoolInput {
                label: "Auto".into(),
                used_percent: 10.0,
                resets_at: Some(at(2026, 8, 1, 0, 0)),
            },
        ],
        resets_at: Some(at(2026, 7, 31, 12, 0)),
        daily: Some(daily(week([1.0; 7]))),
        cost: Some(CursorCostInput {
            metered_cycle_usd: 62.02,
            cycle_tokens: 999_500,
            cycle_input_tokens: 5_000,
            cycle_output_tokens: 12_000_000,
        }),
    }
}

/// Cursor signed-out string and sign-in button.
#[test]
fn test_cursor_signed_out() {
    let p = build_cursor(
        &CursorInput {
            status: signed_out(),
            snapshot: None,
        },
        &ctx(),
    );
    assert_eq!(
        p.message.as_deref(),
        Some("Sign in to cursor.com to load Total, Auto, and API usage.")
    );
    assert_eq!(p.sign_in.unwrap().label, "Sign In to Cursor…");
}

/// Cursor pool tracks, meta label and metric grid.
#[test]
fn test_cursor_usage() {
    let p = build_cursor(
        &CursorInput {
            status: status(),
            snapshot: Some(cursor_snap()),
        },
        &ctx(),
    );
    assert_eq!(p.header.meta.as_deref(), Some("Cursor Ultra"));
    let u = p.usage.unwrap();
    assert_eq!(u.pool_title, "Usage Pool Monthly");
    assert_eq!(u.tracks[0].used_text, "55% Used");
    assert_eq!(
        u.tracks[0].caption.as_deref(),
        Some("Resets Fri 31 July 12:00pm"),
        "falls back to snapshot reset"
    );
    assert_eq!(
        u.tracks[1].caption.as_deref(),
        Some("Resets Sat 01 August 12:00am")
    );
    let m: Vec<(String, String)> = u
        .metrics
        .unwrap()
        .into_iter()
        .map(|m| (m.title, m.value))
        .collect();
    assert_eq!(
        m,
        [
            ("Monthly spend".into(), "$62.02".into()),
            ("Total tokens".into(), "1.0M".into()),
            ("Input tokens".into(), "5K".into()),
            ("Output tokens".into(), "12M".into()),
        ]
    );
}

// ---------------------------------- Claude ---------------------------------

fn claude_code() -> ClaudeCodeInput {
    ClaudeCodeInput {
        month: CodeTotalsInput {
            tokens: 1_500_000,
            cost_usd: 12.5,
            has_unpriced: true,
        },
        all_time: CodeTotalsInput {
            tokens: 2_000_000_000,
            cost_usd: 1234.0,
            has_unpriced: false,
        },
        month_models: vec![
            CodeModelInput {
                name: "Opus 5.5".into(),
                is_unpriced: false,
                cost_usd: 10.0,
                share_percent: 80.0,
            },
            CodeModelInput {
                name: "Mystery".into(),
                is_unpriced: true,
                cost_usd: 0.0,
                share_percent: 0.0,
            },
        ],
        models_expanded: true,
    }
}

fn claude_snap() -> ClaudeSnapshotInput {
    ClaudeSnapshotInput {
        pool_name: "Mixed".into(),
        five_hour: Some(WindowInput {
            label: None,
            used_percent: Some(12.4),
            resets_at: Some(at(2026, 8, 27, 11, 0)),
        }),
        seven_day: Some(WindowInput {
            label: None,
            used_percent: Some(60.0),
            resets_at: None,
        }),
        weekly_label: "Weekly · Opus".into(),
        weekly_resets_at: Some(at(2026, 8, 30, 0, 0)),
        daily: Some(daily(week([1.0; 7]))),
    }
}

/// Claude main state strings.
#[test]
fn test_claude_main() {
    let v = build_claude(
        &ClaudeInput {
            status: status(),
            snapshot: Some(claude_snap()),
            code: Some(claude_code()),
        },
        &ctx(),
    );
    let u = v.panel.usage.unwrap();
    assert_eq!(u.pool_title, "Usage Pool Mixed");
    assert_eq!(u.tracks[0].label, "5-Hour Window");
    assert_eq!(
        u.tracks[0].caption.as_deref(),
        Some("Resets Thu 27 August 11:00am")
    );
    assert_eq!(u.tracks[1].label, "Weekly · Opus");
    assert_eq!(u.tracks[1].used_text, "60% Used");
    assert_eq!(
        u.tracks[1].caption.as_deref(),
        Some("Resets Sun 30 August 12:00am")
    );
    let code = v.code.unwrap();
    let m: Vec<&str> = code.metrics.iter().map(|m| m.value.as_str()).collect();
    assert_eq!(m, ["1.5M", "$12.50+", "2.0B", "$1,234.00"]);
    let titles: Vec<&str> = code.metrics.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Tokens this month",
            "Spend this month",
            "Total tokens",
            "Total spend"
        ]
    );
    let models = code.models.unwrap();
    assert_eq!(models.title, "Models This Month");
    assert_eq!(models.rows[0].cost_text, "$10.00");
    assert_eq!(models.rows[0].share_fraction, 0.8);
    assert_eq!(models.rows[1].cost_text, "No price");
}

/// Claude Code section remains in signed-out and waiting states.
#[test]
fn test_claude_signed_out_keeps_code_section() {
    let v = build_claude(
        &ClaudeInput {
            status: signed_out(),
            snapshot: None,
            code: Some(claude_code()),
        },
        &ctx(),
    );
    assert_eq!(
        v.panel.message.as_deref(),
        Some("Sign in to claude.ai to load your 5-hour and weekly usage.")
    );
    assert_eq!(v.panel.sign_in.unwrap().label, "Sign In to Claude…");
    assert!(v.code.is_some());
    let v = build_claude(
        &ClaudeInput {
            status: status(),
            snapshot: None,
            code: None,
        },
        &ctx(),
    );
    assert_eq!(v.panel.message.as_deref(), Some("No usage data yet."));
    assert!(v.code.is_none());
}

/// Missing windows render 0%; collapsed models hide rows.
#[test]
fn test_claude_missing_windows_and_collapsed_models() {
    let mut snap = claude_snap();
    snap.five_hour = None;
    snap.seven_day = None;
    let mut code = claude_code();
    code.models_expanded = false;
    let v = build_claude(
        &ClaudeInput {
            status: status(),
            snapshot: Some(snap),
            code: Some(code),
        },
        &ctx(),
    );
    let u = v.panel.usage.unwrap();
    assert_eq!(u.tracks[0].used_text, "0% Used");
    assert!(u.tracks[0].caption.is_none());
    assert!(v.code.unwrap().models.unwrap().rows.is_empty());
}

// ---------------------------------- ChatGPT --------------------------------

fn chatgpt_snap() -> ChatGptSnapshotInput {
    ChatGptSnapshotInput {
        plan_name: "ChatGPT Plus".into(),
        pool_name: "Hourly".into(),
        primary: Some(WindowInput {
            label: None,
            used_percent: Some(100.0),
            resets_at: Some(at(2026, 7, 15, 15, 0)),
        }),
        secondary: Some(WindowInput {
            label: Some("Weekly".into()),
            used_percent: Some(20.0),
            resets_at: Some(at(2026, 7, 20, 9, 30)),
        }),
        limit_reached: true,
        allowed: false,
    }
}

/// ChatGPT limit-reached and blocked messages.
#[test]
fn test_chatgpt_limit_reached_and_blocked() {
    let p = build_chatgpt(
        &ChatGptInput {
            status: status(),
            snapshot: Some(chatgpt_snap()),
        },
        &ctx(),
    );
    assert_eq!(p.header.meta.as_deref(), Some("ChatGPT Plus"));
    let u = p.usage.unwrap();
    assert_eq!(u.tracks[0].label, "5-Hour Window");
    assert_eq!(u.tracks[0].caption.as_deref(), Some("Limit reached"));
    assert_eq!(
        u.tracks[1].caption.as_deref(),
        Some("Resets Mon 20 July 9:30am")
    );
    assert_eq!(
        u.blocked_text,
        Some("Codex access is currently blocked for this account.")
    );
    let mut s = chatgpt_snap();
    s.limit_reached = false;
    s.allowed = true;
    s.secondary = None;
    let u = build_chatgpt(
        &ChatGptInput {
            status: status(),
            snapshot: Some(s),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert_eq!(
        u.tracks[0].caption.as_deref(),
        Some("Resets Wed 15 July 3:00pm")
    );
    assert_eq!(u.tracks.len(), 1);
    assert!(u.blocked_text.is_none());
}

/// ChatGPT signed-out.
#[test]
fn test_chatgpt_signed_out() {
    let p = build_chatgpt(
        &ChatGptInput {
            status: signed_out(),
            snapshot: None,
        },
        &ctx(),
    );
    assert_eq!(
        p.message.as_deref(),
        Some("Sign in to chatgpt.com to load your 5-hour and weekly Codex usage.")
    );
    assert_eq!(p.sign_in.unwrap().label, "Sign In to ChatGPT…");
}

// ---------------------------------- OpenCode -------------------------------

fn oc_snap() -> OpenCodeSnapshotInput {
    let m = |name: &str, cost: f64, est: bool, unpriced: bool| OpenCodeModelInput {
        name: name.into(),
        cost_usd: cost,
        is_cost_estimated: est,
        is_cost_unpriced: unpriced,
        percent_of_window: 50.0,
        color: PURPLE,
    };
    OpenCodeSnapshotInput {
        pool_name: "Mixed".into(),
        windows: vec![
            OpenCodeWindowInput {
                kind: OpenCodeWindowKind::FiveHour,
                used_percent: 0.0,
                resets_at: None,
            },
            OpenCodeWindowInput {
                kind: OpenCodeWindowKind::Weekly,
                used_percent: 30.0,
                resets_at: Some(at(2026, 7, 20, 0, 0)),
            },
            OpenCodeWindowInput {
                kind: OpenCodeWindowKind::Monthly,
                used_percent: 80.0,
                resets_at: Some(at(2026, 8, 18, 14, 24)),
            },
        ],
        is_estimated: true,
        daily: Some(daily(week([1.0; 7]))),
        models: vec![
            m("Kimi", 1.5, false, false),
            m("GLM", 0.0, false, true),
            m("Qwen", 2.0, true, false),
            m("X", 1.0, false, false),
        ],
        show_all_models: false,
        models_expanded: true,
        local_stats: Some(OpenCodeLocalStatsInput {
            monthly_usd: 3.0,
            tokens: 3_000,
            input_tokens: 1_000,
            output_tokens: 2_000,
        }),
    }
}

/// OpenCode window captions, colors and notes.
#[test]
fn test_opencode_windows() {
    let p = build_opencode(
        &OpenCodeInput {
            status: status(),
            snapshot: Some(oc_snap()),
        },
        &ctx(),
    );
    assert_eq!(p.header.title, "OpenCode Go");
    let u = p.usage.unwrap();
    assert_eq!(u.tracks[0].label, "5-Hour");
    assert_eq!(u.tracks[0].caption.as_deref(), Some("No usage in last 5h"));
    assert_eq!(u.tracks[0].color, ORANGE);
    assert_eq!(u.tracks[1].caption.as_deref(), Some("Resets Mon 12:00am"));
    assert_eq!(u.tracks[1].color, PURPLE);
    assert_eq!(
        u.tracks[2].caption.as_deref(),
        Some("Resets Tue 18 August 2:24pm")
    );
    assert_eq!(
        u.estimate_note,
        Some("Estimated from local sessions — sign in for console accuracy.")
    );
    let l: Vec<String> = u.metrics.unwrap().into_iter().map(|m| m.value).collect();
    assert_eq!(l, ["$3.00", "3K", "1K", "2K"]);
}

/// 5-hour caption with a reset is relative.
#[test]
fn test_opencode_five_hour_relative_caption() {
    let mut s = oc_snap();
    s.windows[0] = OpenCodeWindowInput {
        kind: OpenCodeWindowKind::FiveHour,
        used_percent: 10.0,
        resets_at: Some(at(2026, 7, 15, 14, 5)),
    };
    let u = build_opencode(
        &OpenCodeInput {
            status: status(),
            snapshot: Some(s),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert_eq!(u.tracks[0].caption.as_deref(), Some("Resets in 2 hours"));
    let mut s = oc_snap();
    s.windows[0].used_percent = 5.0;
    let u = build_opencode(
        &OpenCodeInput {
            status: status(),
            snapshot: Some(s),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert!(u.tracks[0].caption.is_none());
}

/// OpenCode model cost labels and truncation.
#[test]
fn test_opencode_models() {
    let mut s = oc_snap();
    let u = build_opencode(
        &OpenCodeInput {
            status: status(),
            snapshot: Some(s.clone()),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    let card = u.models.unwrap();
    assert_eq!(card.title, "Top Models by Spend");
    assert_eq!(card.rows.len(), 3);
    assert_eq!(card.toggle_label, Some("View all"));
    assert_eq!(card.rows[0].cost_text, "$1.50");
    assert_eq!(card.rows[1].cost_text, "No price");
    assert_eq!(card.rows[2].cost_text, "~$2.00");
    s.show_all_models = true;
    let u = build_opencode(
        &OpenCodeInput {
            status: status(),
            snapshot: Some(s),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert_eq!(u.models.as_ref().unwrap().rows.len(), 4);
    assert_eq!(u.models.unwrap().toggle_label, Some("Show less"));
}

/// OpenCode plain error text, sign-in and signed-out.
#[test]
fn test_opencode_error_and_signed_out() {
    let mut st = signed_out();
    st.last_error = Some("HTTP 500".into());
    let u = build_opencode(
        &OpenCodeInput {
            status: st,
            snapshot: Some(oc_snap()),
        },
        &ctx(),
    )
    .usage
    .unwrap();
    assert_eq!(u.error_text.as_deref(), Some("HTTP 500"));
    assert!(u.footer.stale.is_none());
    assert_eq!(u.sign_in.unwrap().label, "Sign In to OpenCode…");
    assert_eq!(
        u.footer.sign_out.unwrap().confirm_text,
        "Sign out of OpenCode?"
    );
    let p = build_opencode(
        &OpenCodeInput {
            status: signed_out(),
            snapshot: None,
        },
        &ctx(),
    );
    assert_eq!(
        p.message.as_deref(),
        Some(
            "Sign in to the OpenCode console to load official rolling, weekly, and monthly usage."
        )
    );
}

// --------------------------------- OpenRouter ------------------------------

fn or_snap() -> OpenRouterSnapshotInput {
    OpenRouterSnapshotInput {
        budget_source: Some(BudgetSource::AccountCredits),
        budget_usd: Some(100.0),
        used_percent: Some(42.4),
        used_usd: 42.4,
        remaining_usd: Some(57.6),
        account_used_usd: Some(42.4),
        key_usage_usd: 10.0,
        key_usage_daily_usd: 1.25,
        models: vec![OpenRouterModelInput {
            model_id: "z-ai/glm-5.3-flash".into(),
            is_revealed: true,
            cost_usd: 1.0,
            is_cost_estimated: true,
            percent_of_window: 100.0,
        }],
        show_all_models: false,
    }
}

fn or_input(snapshot: Option<OpenRouterSnapshotInput>, status: ProviderStatus) -> OpenRouterInput {
    OpenRouterInput {
        status,
        snapshot,
        key_draft: String::new(),
        is_replacing: false,
        auth_error: None,
    }
}

/// OpenRouter credit budget, stats and models.
#[test]
fn test_openrouter_main() {
    let v = build_openrouter(&or_input(Some(or_snap()), status()), &ctx());
    assert_eq!(v.panel.header.meta.as_deref(), Some("Account credits"));
    let u = v.panel.usage.unwrap();
    assert_eq!(u.budget_title, "Credit Budget");
    assert_eq!(u.used_text.as_deref(), Some("42% Used"));
    let t = u.track.unwrap();
    assert_eq!(t.label, "Credits used");
    assert!(!t.show_label);
    assert_eq!(
        t.caption.as_deref(),
        Some("$42.40 of $100.00 · $57.60 left")
    );
    let stats: Vec<(String, String)> = u.stats.into_iter().map(|m| (m.title, m.value)).collect();
    assert_eq!(
        stats,
        [
            ("Credits purchased".into(), "$100.00".into()),
            ("Total spent".into(), "$42.40".into()),
            ("Remaining".into(), "$57.60".into()),
            ("Spent today".into(), "$1.25".into()),
        ]
    );
    let m = u.models.unwrap();
    assert!(!m.collapsible);
    assert_eq!(m.rows[0].tag.as_deref(), Some("revealed"));
    assert_eq!(m.rows[0].cost_text, "~$1.00");
    assert!(u.replace_link);
    assert!(v.key_entry.is_none());
    assert_eq!(v.sign_out.unwrap().confirm_text, "Sign out of OpenRouter?");
}

/// OpenRouter with no credit limit.
#[test]
fn test_openrouter_no_budget() {
    let mut s = or_snap();
    s.budget_source = None;
    s.budget_usd = None;
    s.used_percent = None;
    s.account_used_usd = None;
    s.remaining_usd = None;
    let v = build_openrouter(&or_input(Some(s), status()), &ctx());
    assert_eq!(v.panel.header.meta.as_deref(), Some("No credit limit"));
    let u = v.panel.usage.unwrap();
    assert!(u.track.is_none() && u.used_text.is_none());
    assert_eq!(
        u.no_limit_text,
        Some("This key has no credit limit, so usage shows as spend only.")
    );
    assert_eq!(u.stats[0].title, "Credit limit");
    assert_eq!(u.stats[0].value, "None");
    assert_eq!(u.stats[1].value, "$10.00", "falls back to key usage");
    assert_eq!(u.stats[2].value, "$0.00");
}

/// Key-limit budget stat title.
#[test]
fn test_openrouter_key_limit_title() {
    let mut s = or_snap();
    s.budget_source = Some(BudgetSource::KeyLimit);
    let u = build_openrouter(&or_input(Some(s), status()), &ctx())
        .panel
        .usage
        .unwrap();
    assert_eq!(u.stats[0].title, "Key spending limit");
}

/// Key entry: signed-out, replace mode, button enabling and error.
#[test]
fn test_openrouter_key_entry() {
    let mut i = or_input(None, signed_out());
    let v = build_openrouter(&i, &ctx());
    assert_eq!(
        v.panel.message.as_deref(),
        Some("Paste an OpenRouter API key to track spending against your purchased credits.")
    );
    let e = v.key_entry.unwrap();
    assert_eq!(
        (e.placeholder, e.button_label, e.button_enabled),
        ("sk-or-v1-…", "Add Key", false)
    );
    i.key_draft = "  sk-or-v1-abc \n".into();
    i.auth_error = Some("Enter an OpenRouter API key.".into());
    let e = build_openrouter(&i, &ctx()).key_entry.unwrap();
    assert!(e.button_enabled);
    assert_eq!(e.error.as_deref(), Some("Enter an OpenRouter API key."));
    i.key_draft = "   ".into();
    assert!(
        !build_openrouter(&i, &ctx())
            .key_entry
            .unwrap()
            .button_enabled
    );

    let mut r = or_input(Some(or_snap()), status());
    r.is_replacing = true;
    let v = build_openrouter(&r, &ctx());
    assert_eq!(v.key_entry.unwrap().button_label, "Save Key");
    assert!(!v.panel.usage.unwrap().replace_link);

    let w = build_openrouter(&or_input(None, status()), &ctx());
    assert_eq!(w.panel.message.as_deref(), Some("No usage data yet."));
    assert!(w.key_entry.is_some());
}
