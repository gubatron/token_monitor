//! Geometry of the sign-in window chrome. The Win32 adapter (`webview2.rs`,
//! excluded from coverage) only moves controls to these rectangles.

use tokenmon_platform::webauth::layout::{
    Bounds, FOOTER_H, HEADER_H, NAV_H, body_bounds, chrome_layout, scaled,
};

/// 96 DPI is 100 %; other DPIs scale and round toward zero.
#[test]
fn test_scaled_follows_dpi() {
    assert_eq!(scaled(72, 96), 72);
    assert_eq!(scaled(72, 144), 108);
    assert_eq!(scaled(10, 120), 12);
    assert_eq!(scaled(1, 120), 1);
    assert_eq!(scaled(-16, 192), -32);
    assert_eq!(scaled(i32::MAX, 192), i32::MAX);
}

/// Negative sizes are clamped so Win32 never gets one.
#[test]
fn test_bounds_clamp_negative_sizes() {
    let b = Bounds::new(5, 6, -10, -1);
    assert_eq!((b.width, b.height), (0, 0));
    assert_eq!((b.right(), b.bottom()), (5, 6));
}

/// At 100 % the default 920x700 client area has the documented layout.
#[test]
fn test_chrome_layout_at_96_dpi() {
    let l = chrome_layout(920, 700, 96);
    assert_eq!(l.title, Bounds::new(16, 10, 920 - 142, 26));
    assert_eq!(l.status, Bounds::new(16, 38, 920 - 142, 30));
    assert_eq!(l.cancel, Bounds::new(920 - 106, 14, 90, 28));
    assert_eq!(l.back, Bounds::new(16, 79, 80, 28));
    assert_eq!(l.forward, Bounds::new(102, 79, 96, 28));
    assert_eq!(l.reload, Bounds::new(204, 79, 80, 28));
    assert_eq!(l.address, Bounds::new(296, 85, 920 - 422, 20));
    assert_eq!(l.close_popup, Bounds::new(920 - 116, 79, 100, 28));
    assert_eq!(l.finish, Bounds::new(920 - 166, 700 - 64 + 16, 150, 32));
    assert_eq!(
        l.separators,
        [
            Bounds::new(0, HEADER_H, 920, 2),
            Bounds::new(0, HEADER_H + NAV_H + 1, 920, 2),
            Bounds::new(0, 700 - FOOTER_H, 920, 2),
        ]
    );
    assert_eq!(l.body, Bounds::new(0, 114, 920, 700 - 65 - 114));
}

/// The web view sits between the navigation bar separator and the footer
/// separator and spans the full width, at any DPI.
#[test]
fn test_body_sits_between_the_chrome() {
    for dpi in [96, 120, 144, 192] {
        let l = chrome_layout(1200, 900, dpi);
        assert!(l.body.y > l.separators[1].y, "dpi {dpi}");
        assert!(l.body.bottom() <= l.separators[2].y, "dpi {dpi}");
        assert_eq!((l.body.x, l.body.width), (0, 1200));
        assert!(l.finish.y > l.body.bottom(), "dpi {dpi}");
        assert!(l.cancel.right() <= 1200, "dpi {dpi}");
    }
}

/// A client area shorter than the chrome gives an empty body, never a
/// negative one, and narrow windows never produce negative widths.
#[test]
fn test_tiny_client_area_never_goes_negative() {
    let body = body_bounds(50, 10, 96);
    assert_eq!(body, Bounds::new(0, 114, 50, 0));
    let l = chrome_layout(50, 10, 144);
    for b in [l.title, l.status, l.address, l.body] {
        assert!(b.width >= 0 && b.height >= 0, "{b:?}");
    }
}
