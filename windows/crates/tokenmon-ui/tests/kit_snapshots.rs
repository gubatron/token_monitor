//! Offscreen PNG snapshot tests of the UI kit (software renderer, bundled
//! Inter font so output does not depend on the host's fonts).
//!
//! Set `UPDATE_SNAPSHOTS=1` to (re)write the committed PNGs under
//! `tests/snapshots/kit/`. Mismatching renders are written next to the cargo
//! target tmp dir as `<name>.actual.png` for inspection.
//!
//! Slint keeps its platform per thread, so everything renders inside one test.
#![allow(missing_docs)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use chrono::NaiveDate;
use slint::fontique_011::{fontique, shared_collection};
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, VecModel};
use tokenmon_core::{ProviderId, Srgb};
use tokenmon_ui::kit::daily::{DailyBudgetDay, DayTokensText, day_bars, range_label};
use tokenmon_ui::kit::palette::ProductColor;
use tokenmon_ui::kit::segments::{ProductShare, layout_segments, to_bar_segments};
use tokenmon_ui::kit::ui::{
    CategoryItem, Gallery, HoverMath, Logos, SnapCaptions, SnapClaudeMock, SnapDailyBudget,
    SnapDailyUsage, SnapMetricGrid, SnapPanelCard, SnapUsage, Theme, Typo,
};

const WIDTH: u32 = 420;
/// Per-channel difference that still counts as equal.
const CHANNEL_TOLERANCE: u8 = 24;
/// Share of pixels allowed to differ beyond the channel tolerance.
const MAX_DIFF_FRACTION: f64 = 0.01;

struct TestPlatform {
    windows: Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        self.windows.borrow_mut().push(w.clone());
        Ok(w)
    }
}

struct Harness {
    windows: Rc<RefCell<Vec<Rc<MinimalSoftwareWindow>>>>,
}

impl Harness {
    fn new() -> Self {
        let windows = Rc::new(RefCell::new(Vec::new()));
        slint::platform::set_platform(Box::new(TestPlatform {
            windows: windows.clone(),
        }))
        .expect("platform set once");
        register_fonts();
        Self { windows }
    }

    /// The window created most recently (the one backing the last component).
    fn latest(&self) -> Rc<MinimalSoftwareWindow> {
        self.windows
            .borrow()
            .last()
            .expect("a window exists")
            .clone()
    }
}

fn register_fonts() {
    let mut collection = shared_collection();
    for bytes in [
        &include_bytes!("fonts/Inter-Regular.ttf")[..],
        &include_bytes!("fonts/Inter-SemiBold.ttf")[..],
    ] {
        collection.register_fonts(fontique::Blob::new(Arc::new(bytes.to_vec())), None);
    }
}

/// Pins the family and theme of a host window.
macro_rules! prepare {
    ($comp:expr, $dark:expr) => {{
        $comp.global::<Typo>().set_family("Inter".into());
        $comp.global::<Theme>().set_dark($dark);
    }};
}

fn render(window: &MinimalSoftwareWindow, height: u32) -> Vec<PremultipliedRgbaColor> {
    window.set_size(PhysicalSize::new(WIDTH, height));
    let mut buf = vec![PremultipliedRgbaColor::default(); (WIDTH * height) as usize];
    slint::platform::update_timers_and_animations();
    let drew = window.draw_if_needed(|r| {
        r.render(&mut buf, WIDTH as usize);
    });
    assert!(drew, "window requested no repaint");
    buf
}

fn rgba(buf: &[PremultipliedRgbaColor]) -> Vec<u8> {
    buf.iter()
        .flat_map(|p| [p.red, p.green, p.blue, p.alpha])
        .collect()
}

fn snapshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/kit")
}

fn write_png(path: &Path, data: &[u8], height: u32) {
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), WIDTH, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(data)
        .expect("png data");
}

fn read_png(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .ok()?;
    let mut out = vec![0; reader.output_buffer_size()];
    reader.next_frame(&mut out).ok()?;
    Some(out)
}

/// Fraction of pixels whose channels differ by more than the tolerance.
fn diff_fraction(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() {
        return 1.0;
    }
    let bad = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .filter(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE)
        })
        .count();
    bad as f64 / (a.len() / 4) as f64
}

/// Renders `window` and compares it with the committed snapshot `name`.
fn check(window: &MinimalSoftwareWindow, name: &str, height: u32, failures: &mut Vec<String>) {
    let data = rgba(&render(window, height));
    let path = snapshot_dir().join(format!("{name}.png"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        write_png(&path, &data, height);
        return;
    }
    match read_png(&path) {
        None => failures.push(format!("{name}: missing snapshot {}", path.display())),
        Some(expected) => {
            let frac = diff_fraction(&data, &expected);
            if frac > MAX_DIFF_FRACTION {
                let actual =
                    Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.png"));
                write_png(&actual, &data, height);
                failures.push(format!(
                    "{name}: {:.2}% pixels differ (see {})",
                    frac * 100.0,
                    actual.display()
                ));
            }
        }
    }
}

fn day(date: NaiveDate, spent: f64, tokens: Option<DayTokensText>) -> DailyBudgetDay {
    let budget = 100.0 / 7.0;
    DailyBudgetDay {
        date,
        spent,
        budget,
        percent_of_budget: spent / budget * 100.0,
        is_over_budget: spent > budget,
        is_prior_window: false,
        tokens,
    }
}

/// Mon Oct 13 .. Sun Oct 19 2025 with today = Thu Oct 16.
fn week() -> (Vec<DailyBudgetDay>, NaiveDate) {
    let d = |n| NaiveDate::from_ymd_opt(2025, 10, n).expect("valid date");
    let tokens = DayTokensText {
        total: "1.2M".into(),
        unit: "tokens".into(),
        cost: "≈ $4.20".into(),
        input: "820K".into(),
        output: "140K".into(),
        cache_read: "210K".into(),
        cache_write: "30K".into(),
        models: vec![
            ("claude-opus-4".into(), "900K".into()),
            ("claude-sonnet".into(), "300K".into()),
        ],
    };
    let mut days = vec![
        day(d(13), 9.0, None),
        day(d(14), 31.0, Some(tokens)),
        day(d(15), 0.0, None),
        day(d(16), 14.3, None),
        day(d(17), 0.0, None),
        day(d(18), 0.0, None),
        day(d(19), 0.0, None),
    ];
    days[0].is_prior_window = true;
    (days, d(16))
}

fn product_shares() -> Vec<ProductShare> {
    let share = |name: &str, p: f64, c: ProductColor| ProductShare {
        name: name.into(),
        percent_of_pool: p,
        color: c.srgb(),
    };
    vec![
        share("Chat", 38.0, ProductColor::Chat),
        share("Build", 21.0, ProductColor::Build),
        share("Voice", 4.0, ProductColor::Voice),
        share("Tiny", 0.01, ProductColor::Api),
    ]
}

fn categories(shares: &[ProductShare]) -> ModelRc<CategoryItem> {
    let items: Vec<CategoryItem> = shares
        .iter()
        .filter(|s| s.percent_of_pool > 0.05)
        .map(|s| CategoryItem {
            name: s.name.as_str().into(),
            percent_text: format!("{}%", s.percent_of_pool.round() as i64).into(),
            color: tokenmon_ui::kit::palette::to_color(s.color),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

#[test]
fn test_kit_component_snapshots() {
    let h = Harness::new();
    let mut failures = Vec::new();
    let (days, today) = week();
    let bars = day_bars(&days, today, "weekly");
    let shares = product_shares();
    let segments = to_bar_segments(&layout_segments(&shares, 396.0, Srgb::rgb(0.5, 0.5, 0.5)));

    let card_segments = to_bar_segments(&layout_segments(&shares, 372.0, Srgb::rgb(0.5, 0.5, 0.5)));
    for (dark, suffix) in [(false, "light"), (true, "dark")] {
        let c = SnapPanelCard::new().expect("SnapPanelCard");
        prepare!(c, dark);
        c.show().expect("show");
        check(
            &h.latest(),
            &format!("panel_card_{suffix}"),
            140,
            &mut failures,
        );

        let c = SnapMetricGrid::new().expect("SnapMetricGrid");
        prepare!(c, dark);
        c.show().expect("show");
        check(
            &h.latest(),
            &format!("metric_grid_{suffix}"),
            168,
            &mut failures,
        );

        let c = SnapUsage::new().expect("SnapUsage");
        prepare!(c, dark);
        c.set_segments(segments.clone());
        c.set_categories(categories(&shares));
        c.show().expect("show");
        check(&h.latest(), &format!("usage_{suffix}"), 150, &mut failures);

        let c = SnapCaptions::new().expect("SnapCaptions");
        prepare!(c, dark);
        c.show().expect("show");
        check(
            &h.latest(),
            &format!("captions_{suffix}"),
            120,
            &mut failures,
        );

        let c = SnapDailyBudget::new().expect("SnapDailyBudget");
        prepare!(c, dark);
        c.set_days(bars.clone());
        c.set_footer("Usage 3.2% over today's allowance".into());
        c.show().expect("show");
        check(
            &h.latest(),
            &format!("daily_budget_{suffix}"),
            260,
            &mut failures,
        );

        let c = SnapDailyUsage::new().expect("SnapDailyUsage");
        prepare!(c, dark);
        c.set_days(bars.clone());
        c.show().expect("show");
        check(
            &h.latest(),
            &format!("daily_usage_{suffix}"),
            250,
            &mut failures,
        );

        let c = Gallery::new().expect("Gallery");
        prepare!(c, dark);
        c.set_days(bars.clone());
        c.set_segments(card_segments.clone());
        c.set_categories(categories(&shares));
        c.show().expect("show");
        check(
            &h.latest(),
            &format!("gallery_{suffix}"),
            620,
            &mut failures,
        );
    }

    // Hover label above the Tuesday stem (token details visible).
    let c = SnapDailyBudget::new().expect("SnapDailyBudget");
    prepare!(c, false);
    c.set_days(bars.clone());
    c.show().expect("show");
    let w = h.latest();
    w.set_size(PhysicalSize::new(WIDTH, 260));
    // Bars row: card padding 12 + 12 window padding; 7 columns of 372/7 px.
    w.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(12.0 + 1.5 * 372.0 / 7.0 + 12.0, 150.0),
    });
    check(&w, "daily_budget_hover", 260, &mut failures);

    // The Claude panel card mock shared on the issue.
    let c = SnapClaudeMock::new().expect("SnapClaudeMock");
    prepare!(c, false);
    c.set_days(bars.clone());
    c.show().expect("show");
    check(&h.latest(), "claude_panel_mock_light", 460, &mut failures);

    assert!(
        failures.is_empty(),
        "snapshot failures:\n{}",
        failures.join("\n")
    );

    // Slint-side geometry agrees with the Rust reference.
    let g = c.global::<HoverMath>();
    assert_eq!(g.invoke_column_index(119.9, 40.0, 7), 2);
    assert_eq!(g.invoke_column_index(280.0, 40.0, 7), -1);
    assert_eq!(g.invoke_column_index(-1.0, 40.0, 7), -1);
    assert_eq!(g.invoke_label_x(200.0, 100.0, 300.0), 86.0);
    assert_eq!(g.invoke_label_x(40.0, 100.0, 300.0), 54.0);
    assert_eq!(g.invoke_label_x(100.0, 250.0, 300.0), 0.0);

    // Theme accents equal the core contract; every logo key resolves.
    let theme = c.global::<Theme>();
    let accents = [
        (ProviderId::Grok, "grok"),
        (ProviderId::Claude, "claude"),
        (ProviderId::OpenCode, "opencode"),
        (ProviderId::Cursor, "cursor"),
        (ProviderId::ChatGpt, "chatgpt"),
        (ProviderId::OpenRouter, "openrouter"),
        (ProviderId::Grokbot, "grokbot"),
        (ProviderId::Overview, "overview"),
    ];
    for (p, key) in accents {
        let got = theme.invoke_accent(key.into());
        assert_eq!(
            (got.red(), got.green(), got.blue()),
            p.accent().to_rgb8(),
            "{key}"
        );
        let panel = theme.invoke_panel_accent(key.into());
        assert!((216..=217).contains(&panel.alpha()), "{key} panel alpha");
        if let Some(logo) = p.logo_asset_key() {
            let size = c.global::<Logos>().invoke_image(logo.into()).size();
            assert!(
                size.width > 0 && size.height > 0,
                "logo {logo} did not load"
            );
        }
    }
    let mark = c
        .global::<Logos>()
        .invoke_image(ProviderId::TOKENMON_MARK_ASSET_KEY.into());
    assert!(mark.size().width > 0, "tokenmon mark did not load");
    let _ = range_label; // re-exported helper is covered in kit_view_models
}
