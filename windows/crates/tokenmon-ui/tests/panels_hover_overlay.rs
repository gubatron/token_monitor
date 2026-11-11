//! The floating hover labels on the real panel window: the daily bars' label
//! (token total, dollars, breakdown, top models) and the Claude "Models This
//! Month" label (month detail and the change against last month).
//!
//! The label used to be a child of the chart's own card, so every card laid
//! out after it painted over the token block. It is now drawn at the window
//! root; these tests hover the real `PanelWindow` and check both the
//! published label and the rendered pixels below the card.
//!
//! Set `HOVER_SNAPSHOT_DIR` to also write the PNGs there (used for the
//! screenshots posted on the issue). Slint keeps its platform per thread, so
//! all scenarios run in one test.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::items_after_statements
)]

mod panels_common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use panels_common::{at, ctx, daily, day, status, week};
use slint::fontique_011::{fontique, shared_collection};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel};
use tokenmon_core::ProviderId;
use tokenmon_core::usage::DayKey;
use tokenmon_core::usage::UsageSource;
use tokenmon_provider_claude::{ClaudeCodeUsageSummary, ModelTokens};
use tokenmon_ui::kit::hover::label_origin_within;
use tokenmon_ui::panels::bridge::{apply_claude, tab_rows};
use tokenmon_ui::panels::claude_chatgpt::{
    ClaudeCodeInput, ClaudeInput, ClaudeSnapshotInput, WindowInput, build_claude,
};
use tokenmon_ui::panels::common::TokensInput;
use tokenmon_ui::panels::switcher::build_switcher;
use tokenmon_ui::panels::ui;

const WIDTH: u32 = 420;

fn register_fonts() {
    let mut collection = shared_collection();
    for bytes in [
        &include_bytes!("fonts/Inter-Regular.ttf")[..],
        &include_bytes!("fonts/Inter-SemiBold.ttf")[..],
    ] {
        collection.register_fonts(fontique::Blob::new(Arc::new(bytes.to_vec())), None);
    }
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

fn render(win: &MinimalSoftwareWindow, height: u32) -> Vec<Rgb8Pixel> {
    let mut buf = vec![Rgb8Pixel::default(); (WIDTH * height) as usize];
    win.draw_if_needed(|r| {
        r.render(&mut buf, WIDTH as usize);
    });
    buf
}

fn save(name: &str, buf: &[Rgb8Pixel], height: u32) {
    let Some(dir) = std::env::var_os("HOVER_SNAPSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    let mut rgba = Vec::with_capacity(buf.len() * 4);
    for p in buf {
        rgba.extend_from_slice(&[p.r, p.g, p.b, 255]);
    }
    let pix =
        tiny_skia::Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(WIDTH, height).unwrap())
            .unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    pix.save_png(dir.join(format!("{name}.png"))).unwrap();
}

/// A realistic Claude Code day: cache-heavy, three models.
fn tokens_of(total_m: f64, cost: f64) -> TokensInput {
    let total = (total_m * 1_000_000.0) as u64;
    TokensInput {
        source: UsageSource::ThisComputer,
        total,
        input: total / 100,
        output: total / 40,
        cache_read: total * 9 / 10,
        cache_write: total - total / 100 - total / 40 - total * 9 / 10,
        cost_usd: cost,
        has_unpriced: false,
        models: vec![
            ("Opus 5.5".into(), total * 7 / 10),
            ("Sonnet 5.5".into(), total * 2 / 10),
            ("Haiku 5.5".into(), total / 10),
        ],
    }
}

fn ledger_day(opus: i64, sonnet: i64, haiku: i64) -> BTreeMap<String, ModelTokens> {
    let m = |out: i64| ModelTokens {
        input_tokens: out / 50,
        output_tokens: out,
        cache_read_tokens: out * 40,
        cache_write_5m_tokens: out * 3,
        response_count: 9,
        ..ModelTokens::default()
    };
    BTreeMap::from([
        ("claude-opus-5-5".to_owned(), m(opus)),
        ("claude-sonnet-5-5".to_owned(), m(sonnet)),
        ("claude-haiku-5-5".to_owned(), m(haiku)),
    ])
}

fn claude_input() -> ClaudeInput {
    let mut days = week([2.0, 5.0, 14.0, 20.0, 0.0, 9.0, 3.0]);
    for (i, (m, c)) in [(31.0, 38.1), (12.4, 14.7), (44.2, 61.9), (58.9, 88.3)]
        .into_iter()
        .enumerate()
    {
        days[i].tokens = Some(tokens_of(m, c));
    }
    let mut d = daily(days);
    d.pace = None;
    // June (last month) is fully recorded; July (this month) in progress.
    let mut ledger = BTreeMap::new();
    for n in 1..=28 {
        ledger.insert(
            DayKey::from_raw(format!("2026-06-{n:02}")),
            ledger_day(300_000 + n * 1_000, 80_000, 20_000),
        );
    }
    for n in 1..=15 {
        ledger.insert(
            DayKey::from_raw(format!("2026-07-{n:02}")),
            ledger_day(420_000 + n * 2_000, 150_000, 25_000),
        );
    }
    let summary = ClaudeCodeUsageSummary::make(&ledger, at(2026, 7, 15, 12, 0));
    ClaudeInput {
        status: status(),
        snapshot: Some(ClaudeSnapshotInput {
            pool_name: "Weekly".into(),
            five_hour: Some(WindowInput {
                label: None,
                used_percent: Some(12.0),
                resets_at: Some(at(2026, 7, 15, 16, 0)),
            }),
            seven_day: Some(WindowInput {
                label: None,
                used_percent: Some(60.0),
                resets_at: Some(at(2026, 7, 20, 0, 0)),
            }),
            weekly_label: "Weekly".into(),
            weekly_resets_at: Some(at(2026, 7, 20, 0, 0)),
            daily: Some(d),
        }),
        code: Some(ClaudeCodeInput::from_summary(&summary, true)),
    }
}

fn hover(win: &MinimalSoftwareWindow, x: f32, y: f32) {
    win.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(x, y),
    });
}

#[test]
fn test_hover_labels_on_the_real_panel() {
    let last = Rc::new(RefCell::new(None));
    slint::platform::set_platform(Box::new(TestPlatform { last: last.clone() })).unwrap();
    register_fonts();
    let c = ctx();
    let tabs = tab_rows(&build_switcher(
        &ProviderId::USAGE_PROVIDERS,
        ProviderId::Claude,
    ));
    let w = ui::PanelWindow::new().unwrap();
    w.set_selected(ui::ProviderTab::Claude);
    w.global::<ui::Typo>().set_family("Inter".into());
    w.set_tabs(tabs);
    apply_claude(
        &w.global::<ui::ClaudeVm>(),
        &build_claude(&claude_input(), &c),
    );
    w.show().unwrap();
    let win = last.borrow().clone().unwrap();
    win.set_size(PhysicalSize::new(WIDTH, 3000));
    slint::platform::update_timers_and_animations();
    let height = w.get_content_height().ceil() as u32;
    win.set_size(PhysicalSize::new(WIDTH, height));
    let plain = render(&win, height);
    let overlay = w.global::<ui::HoverOverlay>();
    assert!(!overlay.get_active(), "nothing is hovered yet");

    // Saturday's bar (third column of the daily card).
    hover(&win, 153.0, 365.0);
    let daily_label = render(&win, height);
    save("win-hover-daily", &daily_label, height);
    assert!(overlay.get_active());
    let card = overlay.get_card();
    assert_eq!(card.title, "Saturday, Jul 11");
    assert_eq!(card.caption, "14.0% of weekly · 98% of daily allowance");
    assert_eq!(
        (card.total.as_str(), card.value.as_str()),
        ("44M", "≈ $61.90")
    );
    let rows = |m: slint::ModelRc<ui::LabelValue>| -> Vec<(String, String)> {
        use slint::Model;
        m.iter()
            .map(|r| (r.label.to_string(), r.value.to_string()))
            .collect()
    };
    let pair = |l: &str, v: &str| (l.to_owned(), v.to_owned());
    assert_eq!(
        rows(card.rows),
        [
            pair("Input", "442K"),
            pair("Output", "1.1M"),
            pair("Cache read", "40M"),
            pair("Cache write", "2.9M"),
        ]
    );
    assert_eq!(
        rows(card.extra_rows),
        [
            pair("Opus 5.5", "31M"),
            pair("Sonnet 5.5", "8.8M"),
            pair("Haiku 5.5", "4.4M"),
        ]
    );

    // A day without tokens (Monday) still gets its date and usage line.
    hover(&win, 266.0, 365.0);
    let card = overlay.get_card();
    assert_eq!(card.title, "Monday, Jul 13");
    assert!(card.total.is_empty() && card.value.is_empty());

    // Opus row of "Models This Month": month detail and the June comparison.
    hover(&win, 100.0, 678.0);
    let model_label = render(&win, height);
    save("win-hover-model", &model_label, height);
    let card = overlay.get_card();
    assert!(overlay.get_active());
    assert_eq!(card.title, "Opus 5.5 · July");
    assert_eq!(
        (card.total.as_str(), card.value.as_str()),
        ("288M", "≈ $281.74")
    );
    assert_eq!(card.note, "June: 388M tokens · ≈ $379.36");
    assert_eq!(card.verdict, "↓ 26% · \u{2212}$97.62 vs June");
    // The label is drawn over the cards above the rows, not under them: the
    // pixels of the stats card differ from the unhovered panel.
    let differs = |from: usize, to: usize| {
        (from * WIDTH as usize..to * WIDTH as usize).any(|i| {
            let (a, b) = (plain[i], model_label[i]);
            (a.r, a.g, a.b) != (b.r, b.g, b.b)
        })
    };
    assert!(differs(538, 618), "label hidden behind the stats card");
    assert!(
        (0..WIDTH as usize).any(|x| {
            let i = 5 * WIDTH as usize + x;
            (plain[i].r, plain[i].g, plain[i].b)
                == (model_label[i].r, model_label[i].g, model_label[i].b)
        }),
        "the top of the window is untouched"
    );

    // The gap between two rows keeps the label up (Swift `nearestRowIndex`).
    hover(&win, 100.0, 695.0);
    assert!(overlay.get_active(), "label dropped in the row gap");
    assert_eq!(overlay.get_card().title, "Opus 5.5 · July");

    // Leaving the rows hides it.
    hover(&win, 210.0, 30.0);
    assert!(!overlay.get_active());

    // The component's vertical placement agrees with the Rust geometry
    // (`label_origin_within`), plus the window clamp.
    let math = w.global::<ui::HoverMath>();
    for (pointer_y, label_h, max_y) in [
        (90.0, 60.0, 100.0),
        (30.0, 60.0, 100.0),
        (20.0, 160.0, 80.0),
    ] {
        let (_, rust_y) =
            label_origin_within((200.0, pointer_y), (100.0, label_h), 300.0, Some(max_y));
        let slint_y = math.invoke_label_y(pointer_y as f32, label_h as f32, max_y as f32, 1000.0);
        assert!(
            (f64::from(slint_y) - rust_y.max(0.0)).abs() < 1e-3,
            "{pointer_y}"
        );
    }
    // Never below the window, never above its top.
    assert_eq!(math.invoke_label_y(990.0, 100.0, 100_000.0, 1000.0), 900.0);
    let _ = day(2026, 7, 9);
}
