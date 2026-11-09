//! Offscreen renders of each Settings tab with the Slint software renderer.
//!
//! Slint allows one platform per process and UI objects are single-threaded, so
//! all four tabs render inside one test. Images are written as PPM files under
//! the cargo target tmp dir for eyeballing; the assertions check that each tab
//! drew real content and that tabs differ.
#![allow(missing_docs)]

mod common;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};
use tokenmon_core::ProviderId;
use tokenmon_ui::settings::bind::create_settings_window;
use tokenmon_ui::settings::{AccountStatus, HostStatus, SettingsViewModel};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 640;

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn render(window: &MinimalSoftwareWindow) -> Vec<PremultipliedRgbaColor> {
    let mut buf = vec![PremultipliedRgbaColor::default(); (WIDTH * HEIGHT) as usize];
    slint::platform::update_timers_and_animations();
    let drew = window.draw_if_needed(|r| {
        r.render(&mut buf, WIDTH as usize);
    });
    assert!(drew, "window requested no repaint");
    buf
}

fn save_ppm(name: &str, buf: &[PremultipliedRgbaColor]) {
    let mut out = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    for p in buf {
        out.extend_from_slice(&[p.red, p.green, p.blue]);
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(dir.join(format!("settings-{name}.ppm")), out).expect("write snapshot");
}

/// Same render as a PNG under the cargo target tmp dir (viewable without a PPM tool).
fn save_png(name: &str, buf: &[PremultipliedRgbaColor]) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let file = std::fs::File::create(dir.join(format!("settings-{name}.png"))).expect("png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), WIDTH, HEIGHT);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = buf
        .iter()
        .flat_map(|p| [p.red, p.green, p.blue, p.alpha])
        .collect();
    enc.write_header()
        .expect("header")
        .write_image_data(&data)
        .expect("data");
}

fn distinct_colors(buf: &[PremultipliedRgbaColor]) -> usize {
    buf.iter()
        .map(|p| (p.red, p.green, p.blue))
        .collect::<HashSet<_>>()
        .len()
}

/// Every tab renders offscreen with non-trivial content, and tabs differ.
#[test]
fn test_each_tab_renders_offscreen() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(TestPlatform {
        window: window.clone(),
    }))
    .expect("platform set once");

    let f = common::fixture(None);
    *f.host.status.lock().unwrap() = HostStatus {
        app_version: Some("1.8.0".into()),
        update_action_title: "Check Now".into(),
        update_can_act: true,
        accounts: vec![AccountStatus {
            provider: ProviderId::Grok,
            signed_in: true,
            email: Some("me@example.com".into()),
            last_error: None,
        }],
        ..HostStatus::default()
    };
    let vm = Arc::new(SettingsViewModel::new(
        f.settings.clone(),
        f.host.clone(),
        f.dialog.clone(),
    ));
    let win = create_settings_window(vm).expect("window");
    win.show().expect("show");

    let mut renders = Vec::new();
    for (index, name) in ["general", "providers", "accounts", "data"]
        .iter()
        .enumerate()
    {
        win.set_current_tab(i32::try_from(index).expect("tab index fits"));
        let buf = render(&window);
        save_ppm(name, &buf);
        save_png(name, &buf);
        assert!(
            distinct_colors(&buf) > 8,
            "{name} tab looks blank ({} colors)",
            distinct_colors(&buf)
        );
        renders.push(
            buf.iter()
                .map(|p| (p.red, p.green, p.blue))
                .collect::<Vec<_>>(),
        );
    }
    for i in 0..renders.len() {
        for j in i + 1..renders.len() {
            assert!(
                renders[i] != renders[j],
                "tabs {i} and {j} rendered identically"
            );
        }
    }
}
