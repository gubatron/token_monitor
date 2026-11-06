//! Offscreen snapshot tests: every panel state is rendered with Slint's
//! software renderer at 420 px wide and checked for sane geometry and
//! non-blank content. PNGs land in `target/panel-snapshots/` for eyeballing
//! (CI uploads them as artifacts).
//!
//! Slint platform state is per process, so all scenarios run sequentially in
//! one test on one thread.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::redundant_closure_for_method_calls,
    clippy::items_after_statements,
    clippy::many_single_char_names
)]

mod panels_common;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use panels_common::{at, ctx, daily, signed_out, status, week};
use slint::fontique_011::{fontique, shared_collection};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize, Rgb8Pixel};
use tokenmon_core::usage::PaceHeadroom;
use tokenmon_core::{ProviderId, Srgb};
use tokenmon_ui::panels::bridge::*;
use tokenmon_ui::panels::claude_chatgpt::*;
use tokenmon_ui::panels::common::*;
use tokenmon_ui::panels::grok::{self, GrokInput, GrokSnapshotInput, ProductInput};
use tokenmon_ui::panels::grokbot_cursor::*;
use tokenmon_ui::panels::opencode_openrouter::*;
use tokenmon_ui::panels::overview::*;
use tokenmon_ui::panels::switcher::build_switcher;
use tokenmon_ui::panels::ui;

/// Registers the bundled Inter fonts so text renders the same everywhere.
fn register_fonts() {
    let mut collection = shared_collection();
    for bytes in [
        &include_bytes!("fonts/Inter-Regular.ttf")[..],
        &include_bytes!("fonts/Inter-SemiBold.ttf")[..],
    ] {
        collection.register_fonts(fontique::Blob::new(Arc::new(bytes.to_vec())), None);
    }
}

/// Pins the font family of a panel window's `Typo` global.
macro_rules! font {
    ($w:expr) => {
        $w.global::<ui::Typo>().set_family("Inter".into())
    };
}

struct TestPlatform {
    last: Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.last.borrow_mut() = Some(w.clone());
        Ok(w)
    }
}

/// Renders `comp` at 420 wide, fitted to its content height; returns RGB bytes and height.
fn render<C: ComponentHandle>(
    comp: &C,
    last: &Rc<RefCell<Option<Rc<MinimalSoftwareWindow>>>>,
    content_height: impl Fn(&C) -> f32,
) -> (Vec<Rgb8Pixel>, u32) {
    comp.show().unwrap();
    let win = last.borrow().clone().unwrap();
    win.set_size(PhysicalSize::new(420, 3000));
    slint::platform::update_timers_and_animations();
    let h = content_height(comp).ceil().max(1.0) as u32;
    win.set_size(PhysicalSize::new(420, h));
    let mut buf = vec![Rgb8Pixel::default(); (420 * h) as usize];
    win.draw_if_needed(|r| {
        r.render(&mut buf, 420);
    });
    (buf, h)
}

fn check_and_save(name: &str, buf: &[Rgb8Pixel], h: u32) {
    assert!((40..=1600).contains(&h), "{name}: implausible height {h}");
    let first = buf[0];
    let distinct = buf
        .iter()
        .filter(|p| (p.r, p.g, p.b) != (first.r, first.g, first.b))
        .count();
    assert!(
        distinct > 200,
        "{name}: render is blank ({distinct} differing pixels)"
    );
    let mut rgba = Vec::with_capacity(buf.len() * 4);
    for p in buf {
        rgba.extend_from_slice(&[p.r, p.g, p.b, 255]);
    }
    let pix =
        tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(420, h).unwrap()).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("panel-snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    pix.save_png(dir.join(format!("{name}.png"))).unwrap();
}

/// `testGridEdgesMatchCardEdgesForEveryProviderCount`: the switcher grid's
/// outermost drawn pixels (row `grid_y`) coincide with a card's (row `card_y`).
fn assert_edges_match(buf: &[Rgb8Pixel], grid_y: usize, card_y: usize) {
    let bg = buf[0];
    let extent = |y: usize| {
        let row = &buf[y * 420..(y + 1) * 420];
        let differs = |p: &&Rgb8Pixel| (p.r, p.g, p.b) != (bg.r, bg.g, bg.b);
        let first = row.iter().position(|p| differs(&p)).unwrap();
        let last = row.iter().rposition(|p| differs(&p)).unwrap();
        (first, last)
    };
    assert_eq!(
        extent(grid_y),
        extent(card_y),
        "switcher grid and card edges differ"
    );
}

fn product(n: &str, p: f64) -> ProductInput {
    ProductInput {
        short_name: n.into(),
        percent: p,
        color: Srgb::rgb(0.2, 0.5, 0.9),
    }
}

fn erroring() -> ProviderStatus {
    ProviderStatus {
        last_error: Some("The network connection was lost.".into()),
        last_refreshed_at: Some(at(2026, 7, 15, 11, 50)),
        ..status()
    }
}

fn refreshing() -> ProviderStatus {
    ProviderStatus {
        is_refreshing: true,
        ..status()
    }
}

fn days() -> Option<DailyBudgetInput> {
    let mut d = daily(week([2.0, 5.0, 14.0, 20.0, 0.0, 9.0, 3.0]));
    d.pace = Some(PaceHeadroom {
        daily_budget: 100.0 / 7.0,
        earned: 30.0,
        period_consumed: 26.0,
        headroom_today: 4.0,
        resets_at: None,
    });
    Some(d)
}

fn win(w: &impl ComponentHandle, name: &str) {
    let _ = (w, name);
}

#[test]
fn test_every_panel_state_renders_offscreen() {
    let last = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform { last: last.clone() })).unwrap();
    register_fonts();
    let c = ctx();
    let tabs = tab_rows(&build_switcher(
        &ProviderId::USAGE_PROVIDERS,
        ProviderId::Grok,
    ));
    let overview_tabs = tab_rows(&build_switcher(
        &ProviderId::USAGE_PROVIDERS,
        ProviderId::Overview,
    ));
    let mut count = 0;

    // Grok
    let gsnap = GrokSnapshotInput {
        used_percent: 36.0,
        resets_at: Some(at(2026, 7, 16, 20, 25)),
        pool_products: vec![product("Chat", 23.0), product("Build", 13.0)],
        category_products: vec![
            product("Chat", 23.0),
            product("Build", 13.0),
            product("Imagine", 0.0),
            product("Voice", 0.0),
        ],
        extra_credits_balance: 4.1,
        week: days(),
    };
    let mut expired = signed_out();
    expired.has_auth_error = true;
    let grok_cases = [
        (
            "grok_signed_out",
            GrokInput {
                status: status(),
                is_signed_in: false,
                snapshot: None,
            },
        ),
        (
            "grok_session_expired",
            GrokInput {
                status: expired,
                is_signed_in: true,
                snapshot: None,
            },
        ),
        (
            "grok_loading",
            GrokInput {
                status: refreshing(),
                is_signed_in: true,
                snapshot: None,
            },
        ),
        (
            "grok_usage",
            GrokInput {
                status: status(),
                is_signed_in: true,
                snapshot: Some(gsnap.clone()),
            },
        ),
        (
            "grok_usage_stale",
            GrokInput {
                status: erroring(),
                is_signed_in: true,
                snapshot: Some(gsnap),
            },
        ),
    ];
    for (name, input) in grok_cases {
        let w = ui::GrokPanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_grok(&w, &grok::build(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // Grokbot
    let bsnap = GrokbotSnapshotInput {
        pool_name: "Weekly".into(),
        used_percent: 42.0,
        entitlement_caption: "Included with SuperGrok".into(),
        has_included_allowance: true,
        resets_at: Some(at(2026, 7, 20, 0, 0)),
        daily: days(),
    };
    let bot_cases = [
        (
            "grokbot_signed_out",
            GrokbotInput {
                status: signed_out(),
                snapshot: None,
            },
        ),
        (
            "grokbot_loading",
            GrokbotInput {
                status: refreshing(),
                snapshot: None,
            },
        ),
        (
            "grokbot_usage",
            GrokbotInput {
                status: status(),
                snapshot: Some(bsnap.clone()),
            },
        ),
        (
            "grokbot_stale_signin_again",
            GrokbotInput {
                status: ProviderStatus {
                    needs_sign_in: true,
                    ..erroring()
                },
                snapshot: Some(bsnap),
            },
        ),
    ];
    for (name, input) in bot_cases {
        let w = ui::GrokbotPanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_grokbot(&w, &build_grokbot(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // Cursor
    let csnap = CursorSnapshotInput {
        plan_name: "Cursor Ultra".into(),
        pools: vec![
            CursorPoolInput {
                label: "Total".into(),
                used_percent: 55.0,
                resets_at: None,
            },
            CursorPoolInput {
                label: "Auto".into(),
                used_percent: 10.0,
                resets_at: None,
            },
        ],
        resets_at: Some(at(2026, 7, 31, 12, 0)),
        daily: days(),
        cost: Some(CursorCostInput {
            metered_cycle_usd: 62.02,
            cycle_tokens: 999_500,
            cycle_input_tokens: 5_000,
            cycle_output_tokens: 12_000_000,
        }),
    };
    let cursor_cases = [
        (
            "cursor_signed_out",
            CursorInput {
                status: signed_out(),
                snapshot: None,
            },
        ),
        (
            "cursor_usage",
            CursorInput {
                status: status(),
                snapshot: Some(csnap.clone()),
            },
        ),
        (
            "cursor_stale",
            CursorInput {
                status: erroring(),
                snapshot: Some(csnap),
            },
        ),
    ];
    for (name, input) in cursor_cases {
        let w = ui::CursorPanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_cursor(&w, &build_cursor(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // Claude
    let code = ClaudeCodeInput {
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
        month_models: vec![CodeModelInput {
            name: "Opus 5.5".into(),
            is_unpriced: false,
            cost_usd: 10.0,
            share_percent: 80.0,
        }],
        models_expanded: true,
    };
    let clsnap = ClaudeSnapshotInput {
        pool_name: "Mixed".into(),
        five_hour: Some(WindowInput {
            label: None,
            used_percent: Some(12.0),
            resets_at: Some(at(2026, 7, 15, 16, 0)),
        }),
        seven_day: Some(WindowInput {
            label: None,
            used_percent: Some(60.0),
            resets_at: None,
        }),
        weekly_label: "Weekly · Opus".into(),
        weekly_resets_at: Some(at(2026, 7, 20, 0, 0)),
        daily: days(),
    };
    let claude_cases = [
        (
            "claude_signed_out_with_code",
            ClaudeInput {
                status: signed_out(),
                snapshot: None,
                code: Some(code.clone()),
            },
        ),
        (
            "claude_loading",
            ClaudeInput {
                status: refreshing(),
                snapshot: None,
                code: None,
            },
        ),
        (
            "claude_usage",
            ClaudeInput {
                status: status(),
                snapshot: Some(clsnap.clone()),
                code: Some(code.clone()),
            },
        ),
        (
            "claude_stale",
            ClaudeInput {
                status: erroring(),
                snapshot: Some(clsnap),
                code: None,
            },
        ),
    ];
    for (name, input) in claude_cases {
        let w = ui::ClaudePanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_claude(&w, &build_claude(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // ChatGPT
    let gpt = ChatGptSnapshotInput {
        plan_name: "ChatGPT Plus".into(),
        pool_name: "Hourly".into(),
        primary: Some(WindowInput {
            label: None,
            used_percent: Some(100.0),
            resets_at: None,
        }),
        secondary: Some(WindowInput {
            label: Some("Weekly".into()),
            used_percent: Some(20.0),
            resets_at: Some(at(2026, 7, 20, 9, 30)),
        }),
        limit_reached: true,
        allowed: false,
    };
    for (name, input) in [
        (
            "chatgpt_signed_out",
            ChatGptInput {
                status: signed_out(),
                snapshot: None,
            },
        ),
        (
            "chatgpt_limit_reached_blocked",
            ChatGptInput {
                status: status(),
                snapshot: Some(gpt),
            },
        ),
    ] {
        let w = ui::ChatGptPanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_chatgpt(&w, &build_chatgpt(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // OpenCode
    let m = |n: &str, cost: f64, est: bool| OpenCodeModelInput {
        name: n.into(),
        cost_usd: cost,
        is_cost_estimated: est,
        is_cost_unpriced: false,
        percent_of_window: 50.0,
        color: PURPLE,
    };
    let ocsnap = OpenCodeSnapshotInput {
        pool_name: "Mixed".into(),
        windows: vec![
            OpenCodeWindowInput {
                kind: OpenCodeWindowKind::FiveHour,
                used_percent: 10.0,
                resets_at: Some(at(2026, 7, 15, 14, 5)),
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
        daily: days(),
        models: vec![
            m("Kimi K2", 1.5, false),
            m("Qwen", 2.0, true),
            m("GLM", 1.0, false),
            m("X", 0.5, false),
        ],
        show_all_models: false,
        models_expanded: true,
        local_stats: Some(OpenCodeLocalStatsInput {
            monthly_usd: 3.0,
            tokens: 3_000,
            input_tokens: 1_000,
            output_tokens: 2_000,
        }),
    };
    for (name, input) in [
        (
            "opencode_signed_out",
            OpenCodeInput {
                status: signed_out(),
                snapshot: None,
            },
        ),
        (
            "opencode_usage",
            OpenCodeInput {
                status: status(),
                snapshot: Some(ocsnap.clone()),
            },
        ),
        (
            "opencode_error_needs_sign_in",
            OpenCodeInput {
                status: ProviderStatus {
                    needs_sign_in: true,
                    ..erroring()
                },
                snapshot: Some(ocsnap),
            },
        ),
    ] {
        let w = ui::OpenCodePanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_opencode(&w, &build_opencode(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // OpenRouter
    let orsnap = OpenRouterSnapshotInput {
        budget_source: Some(BudgetSource::AccountCredits),
        budget_usd: Some(100.0),
        used_percent: Some(42.0),
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
    };
    let nolimit = OpenRouterSnapshotInput {
        budget_source: None,
        budget_usd: None,
        used_percent: None,
        remaining_usd: None,
        account_used_usd: None,
        ..orsnap.clone()
    };
    let base = |snapshot, status| OpenRouterInput {
        status,
        snapshot,
        key_draft: String::new(),
        is_replacing: false,
        auth_error: None,
    };
    for (name, input) in [
        (
            "openrouter_signed_out_key_entry",
            OpenRouterInput {
                auth_error: Some(
                    "That doesn't look like an OpenRouter key (expected sk-or-…).".into(),
                ),
                ..base(None, signed_out())
            },
        ),
        ("openrouter_usage", base(Some(orsnap.clone()), status())),
        ("openrouter_no_credit_limit", base(Some(nolimit), status())),
        (
            "openrouter_replacing_key",
            OpenRouterInput {
                is_replacing: true,
                key_draft: "sk-or-v1-x".into(),
                ..base(Some(orsnap), status())
            },
        ),
    ] {
        let w = ui::OpenRouterPanel::new().unwrap();
        font!(w);
        w.set_tabs(tabs.clone());
        apply_openrouter(&w, &build_openrouter(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        count += 1;
    }

    // Overview
    let mut hours = vec![Vec::new(); 24];
    hours[9] = vec![
        HourActivityInput {
            series: HourSeries::Claude,
            quota_percent: 2.0,
            tokens: Some(1_500_000),
        },
        HourActivityInput {
            series: HourSeries::Grok,
            quota_percent: 1.0,
            tokens: Some(500),
        },
    ];
    hours[14] = vec![HourActivityInput {
        series: HourSeries::OpenCodeGo,
        quota_percent: 3.0,
        tokens: None,
    }];
    let mut cards: Vec<OverviewCardInput> = ProviderId::USAGE_PROVIDERS
        .iter()
        .map(|p| OverviewCardInput {
            provider: *p,
            has_snapshot: false,
            percent: None,
            reset_at: None,
            signed_in: true,
            needs_sign_in: false,
            key_usage_usd: None,
            has_budget_source: false,
            remaining_usd: None,
        })
        .collect();
    cards[0].has_snapshot = true;
    cards[0].percent = Some(36.0);
    cards[0].reset_at = Some(at(2026, 7, 16, 20, 25));
    cards[3].needs_sign_in = true;
    for (name, hrs, off) in [
        ("overview_populated", hours, 0),
        ("overview_empty_yesterday", vec![Vec::new(); 24], -1),
    ] {
        let input = OverviewInput {
            version: Some("1.10.1".into()),
            cards: cards.clone(),
            daily: OverviewDailyInput {
                day_offset: off,
                hours: hrs,
                earliest_day: None,
            },
        };
        let w = ui::OverviewPanel::new().unwrap();
        font!(w);
        w.set_tabs(overview_tabs.clone());
        apply_overview(&w, &build_overview(&input, &c));
        let (buf, h) = render(&w, &last, |w| w.get_content_height());
        check_and_save(name, &buf, h);
        if name == "overview_populated" {
            assert_edges_match(&buf, 26, 150);
        }
        count += 1;
    }
    win(&ui::OverviewPanel::new().unwrap(), "unused");
    assert!(count == 27, "expected the full scenario table, ran {count}");
}
