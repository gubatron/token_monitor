//! Where the tray panel goes: a pure function from the tray icon's rectangle,
//! the monitor's work area, the taskbar edge, the monitor's DPI and the
//! panel's logical size to the panel's top-left corner (#81).
//!
//! Everything here is in **physical pixels** of the virtual screen (what
//! Win32 and the tray icon report), except the panel's *logical* size, which
//! is what the Slint layout produces and is converted with the monitor's DPI.
//! Mixing the two is how a panel ends up half off-screen at 125% and above,
//! so the conversion lives in exactly one place ([`scale_px`]).
//!
//! The Windows glue (`shell`) only gathers the inputs and applies the result;
//! it is logged with `--debug-panel` so a bad placement can be reproduced here
//! as a unit test.

use crate::tray::Rect;

/// Pixels per inch at 100% scaling.
pub const BASE_DPI: u32 = 96;

/// Gap between the taskbar and the panel, in logical pixels.
pub const PANEL_GAP_DIP: i32 = 8;

/// Side of the screen the taskbar is docked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarEdge {
    /// Taskbar along the bottom (the Windows default).
    Bottom,
    /// Taskbar along the top.
    Top,
    /// Taskbar along the left.
    Left,
    /// Taskbar along the right.
    Right,
}

/// Everything the placement depends on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementInput {
    /// The tray icon's rectangle (physical pixels, virtual screen).
    pub icon: Rect,
    /// The whole monitor the icon is on.
    pub monitor: Rect,
    /// The monitor's work area (the monitor minus the taskbar).
    pub work: Rect,
    /// The monitor's effective DPI (96 = 100%).
    pub dpi: u32,
    /// The panel's size in logical pixels.
    pub logical_size: (f32, f32),
}

/// The decision, with the intermediate numbers kept for `--debug-panel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The taskbar edge the panel opened away from.
    pub edge: TaskbarEdge,
    /// The panel's physical size.
    pub size: (i32, i32),
    /// The gap used, in physical pixels.
    pub gap: i32,
    /// Top-left corner of the panel, physical pixels.
    pub origin: (i32, i32),
}

/// `logical` pixels at `dpi`, rounded up so the panel never ends up smaller
/// than the window Slint will create.
///
/// ```
/// use tokenmon_app::placement::scale_px;
/// assert_eq!(scale_px(420.0, 96), 420);
/// assert_eq!(scale_px(420.0, 144), 630);
/// assert_eq!(scale_px(8.0, 120), 10);
/// ```
pub fn scale_px(logical: f32, dpi: u32) -> i32 {
    let dpi = if dpi == 0 { BASE_DPI } else { dpi };
    (logical * dpi as f32 / BASE_DPI as f32).ceil() as i32
}

fn right(r: Rect) -> i32 {
    r.x + r.width
}

fn bottom(r: Rect) -> i32 {
    r.y + r.height
}

/// Which edge the taskbar is on: the side where the work area is inset from
/// the monitor the most. With an auto-hidden taskbar there is no inset, so the
/// monitor edge closest to the icon wins. Ties prefer bottom, top, left, right.
///
/// ```
/// use tokenmon_app::placement::{TaskbarEdge, taskbar_edge};
/// use tokenmon_app::tray::Rect;
/// let monitor = Rect { x: 0, y: 0, width: 1920, height: 1080 };
/// let work = Rect { x: 0, y: 0, width: 1920, height: 1040 };
/// let icon = Rect { x: 1800, y: 1048, width: 24, height: 24 };
/// assert_eq!(taskbar_edge(monitor, work, icon), TaskbarEdge::Bottom);
/// ```
pub fn taskbar_edge(monitor: Rect, work: Rect, icon: Rect) -> TaskbarEdge {
    let insets = [
        (TaskbarEdge::Bottom, bottom(monitor) - bottom(work)),
        (TaskbarEdge::Top, work.y - monitor.y),
        (TaskbarEdge::Left, work.x - monitor.x),
        (TaskbarEdge::Right, right(monitor) - right(work)),
    ];
    let largest = insets.iter().map(|(_, inset)| *inset).max().unwrap_or(0);
    if largest > 0 {
        return insets
            .iter()
            .find(|(_, inset)| *inset == largest)
            .map_or(TaskbarEdge::Bottom, |(edge, _)| *edge);
    }
    let (cx, cy) = (icon.x + icon.width / 2, icon.y + icon.height / 2);
    let distances = [
        (TaskbarEdge::Bottom, bottom(monitor) - cy),
        (TaskbarEdge::Top, cy - monitor.y),
        (TaskbarEdge::Left, cx - monitor.x),
        (TaskbarEdge::Right, right(monitor) - cx),
    ];
    let nearest = distances.iter().map(|(_, d)| *d).min().unwrap_or(0);
    distances
        .iter()
        .find(|(_, d)| *d == nearest)
        .map_or(TaskbarEdge::Bottom, |(edge, _)| *edge)
}

/// A stand-in icon rectangle when no tray event has reported one yet (a toast
/// click or `--show` right at startup): the corner of the taskbar nearest the
/// notification area, at the small-icon size for `dpi`.
///
/// ```
/// use tokenmon_app::placement::fallback_icon;
/// use tokenmon_app::tray::Rect;
/// let monitor = Rect { x: 0, y: 0, width: 1920, height: 1080 };
/// let work = Rect { x: 0, y: 0, width: 1920, height: 1040 };
/// let icon = fallback_icon(monitor, work, 96);
/// assert_eq!((icon.x, icon.y), (1856, 1040));
/// ```
pub fn fallback_icon(monitor: Rect, work: Rect, dpi: u32) -> Rect {
    let side = scale_px(16.0, dpi);
    let margin = scale_px(64.0, dpi);
    let probe = Rect {
        x: right(monitor),
        y: bottom(monitor),
        width: 0,
        height: 0,
    };
    let (x, y) = match taskbar_edge(monitor, work, probe) {
        TaskbarEdge::Bottom => (right(monitor) - margin, bottom(work)),
        TaskbarEdge::Top => (right(monitor) - margin, work.y - side),
        TaskbarEdge::Left => (work.x - side, bottom(monitor) - margin),
        TaskbarEdge::Right => (right(work), bottom(monitor) - margin),
    };
    Rect {
        x,
        y,
        width: side,
        height: side,
    }
}

fn clamp_axis(start: i32, len: i32, lo: i32, extent: i32) -> i32 {
    start.clamp(lo, (lo + extent - len).max(lo))
}

/// The panel's top-left corner for `input`: opened away from the taskbar,
/// centred on the icon along the taskbar, a DPI-scaled gap off the taskbar
/// side of the work area, then clamped entirely inside the work area. A panel
/// larger than the work area pins to its top left.
///
/// ```
/// use tokenmon_app::placement::{PlacementInput, place_panel};
/// use tokenmon_app::tray::Rect;
/// let p = place_panel(&PlacementInput {
///     icon: Rect { x: 1800, y: 1048, width: 24, height: 24 },
///     monitor: Rect { x: 0, y: 0, width: 1920, height: 1080 },
///     work: Rect { x: 0, y: 0, width: 1920, height: 1040 },
///     dpi: 96,
///     logical_size: (420.0, 600.0),
/// });
/// assert_eq!(p.origin, (1500, 432));
/// ```
pub fn place_panel(input: &PlacementInput) -> Placement {
    let PlacementInput {
        icon,
        monitor,
        work,
        dpi,
        logical_size,
    } = *input;
    let size = (scale_px(logical_size.0, dpi), scale_px(logical_size.1, dpi));
    let gap = scale_px(PANEL_GAP_DIP as f32, dpi);
    let edge = taskbar_edge(monitor, work, icon);
    let (cx, cy) = (icon.x + icon.width / 2, icon.y + icon.height / 2);
    let (x, y) = match edge {
        TaskbarEdge::Bottom => (cx - size.0 / 2, icon.y.min(bottom(work)) - gap - size.1),
        TaskbarEdge::Top => (cx - size.0 / 2, bottom(icon).max(work.y) + gap),
        TaskbarEdge::Left => (right(icon).max(work.x) + gap, cy - size.1 / 2),
        TaskbarEdge::Right => (icon.x.min(right(work)) - gap - size.0, cy - size.1 / 2),
    };
    Placement {
        edge,
        size,
        gap,
        origin: (
            clamp_axis(x, size.0, work.x, work.width),
            clamp_axis(y, size.1, work.y, work.height),
        ),
    }
}

/// Where the anchor rectangle came from (logged with `--debug-panel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorSource {
    /// The tray icon's own rectangle.
    Icon,
    /// The taskbar's notification area (the icon is in the overflow flyout or
    /// the OS gave no rectangle).
    NotificationArea,
    /// Neither was available: the corner of the taskbar.
    Fallback,
}

fn usable(r: Rect) -> bool {
    r.width > 0 && r.height > 0
}

fn contains(outer: Rect, x: i32, y: i32) -> bool {
    x >= outer.x && x < right(outer) && y >= outer.y && y < bottom(outer)
}

/// Picks the rectangle the panel anchors to.
///
/// On Windows 10 a new tray icon starts in the overflow flyout (the `^`
/// popup). A click there reports a rectangle inside the flyout, which closes
/// the moment the click lands, so anchoring to it leaves the panel floating
/// where the flyout was. When the icon rectangle is empty or its centre is
/// inside the visible `overflow` flyout, anchor to the `notify_area`
/// (`TrayNotifyWnd`) instead, and with neither fall back to the taskbar
/// corner.
///
/// ```
/// use tokenmon_app::placement::{AnchorSource, resolve_anchor};
/// use tokenmon_app::tray::Rect;
/// let monitor = Rect { x: 0, y: 0, width: 1920, height: 1080 };
/// let work = Rect { x: 0, y: 0, width: 1920, height: 1040 };
/// let flyout = Rect { x: 1600, y: 900, width: 140, height: 130 };
/// let icon = Rect { x: 1650, y: 950, width: 16, height: 16 };
/// let tray = Rect { x: 1700, y: 1040, width: 220, height: 40 };
/// let (rect, source) = resolve_anchor(Some(icon), Some(flyout), Some(tray), monitor, work, 96);
/// assert_eq!((rect, source), (tray, AnchorSource::NotificationArea));
/// ```
pub fn resolve_anchor(
    icon: Option<Rect>,
    overflow: Option<Rect>,
    notify_area: Option<Rect>,
    monitor: Rect,
    work: Rect,
    dpi: u32,
) -> (Rect, AnchorSource) {
    let in_flyout =
        |i: Rect| overflow.is_some_and(|o| contains(o, i.x + i.width / 2, i.y + i.height / 2));
    if let Some(i) = icon.filter(|i| usable(*i) && !in_flyout(*i)) {
        return (i, AnchorSource::Icon);
    }
    if let Some(n) = notify_area.filter(|n| usable(*n)) {
        return (n, AnchorSource::NotificationArea);
    }
    (fallback_icon(monitor, work, dpi), AnchorSource::Fallback)
}
