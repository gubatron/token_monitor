//! Geometry of the sign-in window's native chrome (header, navigation bar,
//! footer) and of the web view area between them, in physical pixels.
//!
//! Portable so it is tested on every OS; the `cfg(windows)` adapter
//! (`webview2`) only moves its controls to these rectangles.

/// Header height (title, status, Cancel) at 96 DPI.
pub const HEADER_H: i32 = 72;
/// Navigation bar height (Back, Forward, Reload, address) at 96 DPI.
pub const NAV_H: i32 = 40;
/// Footer height (Finish) at 96 DPI.
pub const FOOTER_H: i32 = 64;

/// A rectangle as Win32 `MoveWindow` takes it: origin plus size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width (never negative).
    pub width: i32,
    /// Height (never negative).
    pub height: i32,
}

impl Bounds {
    /// A rectangle whose negative width or height is clamped to 0 (a window
    /// narrower than the chrome must not pass a negative size to Win32).
    #[must_use]
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width: width.max(0),
            height: height.max(0),
        }
    }

    /// Right edge (`x + width`).
    #[must_use]
    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    /// Bottom edge (`y + height`).
    #[must_use]
    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }
}

/// `v` device-independent pixels at `dpi` (96 is 100 %), rounded toward zero.
#[must_use]
pub fn scaled(v: i32, dpi: u32) -> i32 {
    let px = i64::from(v) * i64::from(dpi) / 96;
    i32::try_from(px).unwrap_or(if px < 0 { i32::MIN } else { i32::MAX })
}

/// Where every chrome control and the web view go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeLayout {
    /// Header title.
    pub title: Bounds,
    /// Header status line.
    pub status: Bounds,
    /// Cancel button (header, right).
    pub cancel: Bounds,
    /// Back button.
    pub back: Bounds,
    /// Forward button.
    pub forward: Bounds,
    /// Reload button.
    pub reload: Bounds,
    /// Address label.
    pub address: Bounds,
    /// "Close popup" button (navigation bar, right).
    pub close_popup: Bounds,
    /// Finish button (footer, right).
    pub finish: Bounds,
    /// Separators below the header, below the navigation bar and above the footer.
    pub separators: [Bounds; 3],
    /// The web view area between the navigation bar and the footer.
    pub body: Bounds,
}

/// The web view area for a client area of `width` x `height` at `dpi`: below
/// the navigation bar, above the footer, never with a negative height.
#[must_use]
pub fn body_bounds(width: i32, height: i32, dpi: u32) -> Bounds {
    let top = scaled(HEADER_H + NAV_H + 2, dpi);
    let bottom = (height - scaled(FOOTER_H + 1, dpi)).max(top);
    Bounds::new(0, top, width, bottom - top)
}

/// Lays out the chrome for a client area of `width` x `height` at `dpi`.
#[must_use]
pub fn chrome_layout(width: i32, height: i32, dpi: u32) -> ChromeLayout {
    let s = |v: i32| scaled(v, dpi);
    let (w, h) = (width, height);
    let ny = s(HEADER_H + 1 + 6);
    ChromeLayout {
        title: Bounds::new(s(16), s(10), w - s(16 + 16 + 110), s(26)),
        status: Bounds::new(s(16), s(38), w - s(16 + 16 + 110), s(30)),
        cancel: Bounds::new(w - s(16 + 90), s(14), s(90), s(28)),
        back: Bounds::new(s(16), ny, s(80), s(28)),
        forward: Bounds::new(s(102), ny, s(96), s(28)),
        reload: Bounds::new(s(204), ny, s(80), s(28)),
        address: Bounds::new(s(296), ny + s(6), w - s(296 + 16 + 110), s(20)),
        close_popup: Bounds::new(w - s(16 + 100), ny, s(100), s(28)),
        finish: Bounds::new(w - s(16 + 150), h - s(FOOTER_H) + s(16), s(150), s(32)),
        separators: [
            Bounds::new(0, s(HEADER_H), w, 2),
            Bounds::new(0, s(HEADER_H + NAV_H + 1), w, 2),
            Bounds::new(0, h - s(FOOTER_H), w, 2),
        ],
        body: body_bounds(w, h, dpi),
    }
}
