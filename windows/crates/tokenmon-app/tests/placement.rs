//! Tray panel placement (#81): the panel opens away from the taskbar on every
//! edge, at every DPI scale, on monitors at any offset, and never leaves the
//! work area.

use tokenmon_app::placement::{
    AnchorSource, PlacementInput, TaskbarEdge, fallback_icon, place_panel, resolve_anchor,
    scale_px, taskbar_edge,
};
use tokenmon_app::tray::Rect;

const SCALES: [(u32, f32); 4] = [(96, 1.0), (120, 1.25), (144, 1.5), (192, 2.0)];
const EDGES: [TaskbarEdge; 4] = [
    TaskbarEdge::Bottom,
    TaskbarEdge::Top,
    TaskbarEdge::Left,
    TaskbarEdge::Right,
];

fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// A `w`x`h` monitor at `(ox, oy)` with a `bar` px taskbar on `edge`.
fn monitor_with_bar(ox: i32, oy: i32, w: i32, h: i32, edge: TaskbarEdge, bar: i32) -> (Rect, Rect) {
    let monitor = rect(ox, oy, w, h);
    let work = match edge {
        TaskbarEdge::Bottom => rect(ox, oy, w, h - bar),
        TaskbarEdge::Top => rect(ox, oy + bar, w, h - bar),
        TaskbarEdge::Left => rect(ox + bar, oy, w - bar, h),
        TaskbarEdge::Right => rect(ox, oy, w - bar, h),
    };
    (monitor, work)
}

/// The tray icon in the taskbar, 300 px from the screen corner.
fn icon_on(edge: TaskbarEdge, monitor: Rect, bar: i32, side: i32) -> Rect {
    let pad = (bar - side) / 2;
    let (r, b) = (monitor.x + monitor.width, monitor.y + monitor.height);
    match edge {
        TaskbarEdge::Bottom => rect(r - 300, b - bar + pad, side, side),
        TaskbarEdge::Top => rect(r - 300, monitor.y + pad, side, side),
        TaskbarEdge::Left => rect(monitor.x + pad, b - 300, side, side),
        TaskbarEdge::Right => rect(r - bar + pad, b - 300, side, side),
    }
}

/// The panel is inside the work area on each axis, or, when it is larger than
/// the work area on that axis (a tall panel at 200% on a 1080p screen),
/// pinned to its start.
fn inside(origin: (i32, i32), size: (i32, i32), work: Rect) -> bool {
    let axis = |start: i32, len: i32, lo: i32, extent: i32| {
        if len <= extent {
            start >= lo && start + len <= lo + extent
        } else {
            start == lo
        }
    };
    axis(origin.0, size.0, work.x, work.width) && axis(origin.1, size.1, work.y, work.height)
}

/// DPI scaling rounds up and treats an unknown DPI as 100%.
#[test]
fn test_scale_px_rounds_up_and_defaults_to_100_percent() {
    assert_eq!(scale_px(420.0, 96), 420);
    assert_eq!(scale_px(420.0, 120), 525);
    assert_eq!(scale_px(420.0, 144), 630);
    assert_eq!(scale_px(420.0, 192), 840);
    assert_eq!(scale_px(8.0, 120), 10);
    assert_eq!(scale_px(420.0, 0), 420);
    assert_eq!(scale_px(100.4, 96), 101);
}

/// The reported symptom: a default Windows layout (bottom taskbar, icon at
/// the right) opens the panel ABOVE the icon, not at the bottom right
/// off-screen.
#[test]
fn test_bottom_taskbar_opens_above_the_icon_at_100_percent() {
    let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, TaskbarEdge::Bottom, 48);
    let p = place_panel(&PlacementInput {
        icon: rect(1650, 1052, 24, 24),
        monitor,
        work,
        dpi: 96,
        logical_size: (420.0, 600.0),
    });
    assert_eq!(p.edge, TaskbarEdge::Bottom);
    assert_eq!(p.size, (420, 600));
    // The panel's bottom sits `gap` above the taskbar (work bottom = 1032).
    assert_eq!(p.origin, (1650 + 12 - 210, 1032 - 8 - 600));
}

/// A panel whose icon is at the far right is clamped, not pushed off-screen.
#[test]
fn test_icon_at_the_right_edge_clamps_into_the_work_area() {
    let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, TaskbarEdge::Bottom, 48);
    let p = place_panel(&PlacementInput {
        icon: rect(1890, 1052, 24, 24),
        monitor,
        work,
        dpi: 96,
        logical_size: (420.0, 600.0),
    });
    assert_eq!(p.origin.0, 1920 - 420);
}

/// Every taskbar edge, every scale, primary and secondary monitors (including
/// negative virtual-screen coordinates): the panel is fully inside the work
/// area and on the side away from the taskbar.
#[test]
fn test_panel_stays_inside_the_work_area_and_away_from_the_taskbar() {
    let monitors = [
        (0, 0, 1920, 1080),
        (-2560, 0, 2560, 1440),
        (0, -1080, 1920, 1080),
        (-3840, -200, 3840, 2160),
        (1920, 300, 1366, 768),
    ];
    for edge in EDGES {
        for (ox, oy, w, h) in monitors {
            for (dpi, scale) in SCALES {
                let bar = (48.0 * scale) as i32;
                let (monitor, work) = monitor_with_bar(ox, oy, w, h, edge, bar);
                let icon = icon_on(edge, monitor, bar, (24.0 * scale) as i32);
                let p = place_panel(&PlacementInput {
                    icon,
                    monitor,
                    work,
                    dpi,
                    logical_size: (420.0, 620.0),
                });
                let ctx = format!("{edge:?} {ox},{oy} {w}x{h} dpi {dpi}: {p:?}");
                assert_eq!(p.edge, edge, "{ctx}");
                assert_eq!(
                    p.size,
                    (scale_px(420.0, dpi), scale_px(620.0, dpi)),
                    "{ctx}"
                );
                assert!(inside(p.origin, p.size, work), "{ctx}");
                let (x, y) = p.origin;
                if p.size.0 > work.width || p.size.1 > work.height {
                    // Cannot fit (tall panel, 200%, 1080p): pinned, checked above.
                    continue;
                }
                match edge {
                    TaskbarEdge::Bottom => assert!(y + p.size.1 <= icon.y, "{ctx}"),
                    TaskbarEdge::Top => assert!(y >= icon.y + icon.height, "{ctx}"),
                    TaskbarEdge::Left => assert!(x >= icon.x + icon.width, "{ctx}"),
                    TaskbarEdge::Right => assert!(x + p.size.0 <= icon.x, "{ctx}"),
                }
            }
        }
    }
}

/// The gap scales with DPI so the panel is the same distance from the taskbar
/// in logical terms.
#[test]
fn test_gap_scales_with_dpi() {
    for (dpi, scale) in SCALES {
        let bar = (48.0 * scale) as i32;
        let (monitor, work) = monitor_with_bar(0, 0, 3840, 2160, TaskbarEdge::Bottom, bar);
        let icon = icon_on(TaskbarEdge::Bottom, monitor, bar, (24.0 * scale) as i32);
        let p = place_panel(&PlacementInput {
            icon,
            monitor,
            work,
            dpi,
            logical_size: (420.0, 500.0),
        });
        assert_eq!(p.gap, (8.0 * scale).ceil() as i32);
        assert_eq!(p.origin.1 + p.size.1 + p.gap, work.y + work.height);
    }
}

/// Left and right taskbars centre the panel on the icon vertically and open
/// sideways.
#[test]
fn test_side_taskbars_open_sideways() {
    let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, TaskbarEdge::Left, 64);
    let p = place_panel(&PlacementInput {
        icon: rect(20, 500, 24, 24),
        monitor,
        work,
        dpi: 96,
        logical_size: (420.0, 600.0),
    });
    assert_eq!(p.origin, (64 + 8, 512 - 300));
    let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, TaskbarEdge::Right, 64);
    let p = place_panel(&PlacementInput {
        icon: rect(1880, 500, 24, 24),
        monitor,
        work,
        dpi: 96,
        logical_size: (420.0, 600.0),
    });
    assert_eq!(p.origin, (1856 - 8 - 420, 512 - 300));
}

/// A panel taller than the work area pins to the top instead of going
/// negative.
#[test]
fn test_oversized_panel_pins_to_the_work_area_origin() {
    let (monitor, work) = monitor_with_bar(-1366, 100, 1366, 768, TaskbarEdge::Bottom, 40);
    let p = place_panel(&PlacementInput {
        icon: rect(-300, 830, 20, 20),
        monitor,
        work,
        dpi: 96,
        logical_size: (420.0, 5000.0),
    });
    assert_eq!(p.origin.1, work.y);
}

/// Edge detection uses the work-area inset; an auto-hidden taskbar (no inset)
/// falls back to the nearest monitor edge to the icon.
#[test]
fn test_taskbar_edge_detection() {
    let monitor = rect(-1920, 0, 1920, 1080);
    for edge in EDGES {
        let (m, work) = monitor_with_bar(-1920, 0, 1920, 1080, edge, 48);
        assert_eq!(m, monitor);
        assert_eq!(taskbar_edge(m, work, rect(-100, 500, 20, 20)), edge);
    }
    let cases = [
        (rect(-1000, 1070, 20, 20), TaskbarEdge::Bottom),
        (rect(-1000, 5, 20, 20), TaskbarEdge::Top),
        (rect(-1915, 500, 20, 20), TaskbarEdge::Left),
        (rect(-10, 500, 20, 20), TaskbarEdge::Right),
    ];
    for (icon, edge) in cases {
        assert_eq!(taskbar_edge(monitor, monitor, icon), edge);
    }
}

/// Before any tray event the fallback icon sits at the notification-area end
/// of the taskbar, so the panel still opens beside the taskbar.
#[test]
fn test_fallback_icon_places_the_panel_near_the_taskbar() {
    for edge in EDGES {
        for (dpi, scale) in SCALES {
            let bar = (48.0 * scale) as i32;
            let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, edge, bar);
            let icon = fallback_icon(monitor, work, dpi);
            let p = place_panel(&PlacementInput {
                icon,
                monitor,
                work,
                dpi,
                logical_size: (420.0, 600.0),
            });
            assert_eq!(p.edge, edge);
            assert!(inside(p.origin, p.size, work), "{edge:?} {dpi}: {p:?}");
        }
    }
}

/// The maintainer's Windows 10 layout: the icon is in the overflow flyout
/// above the taskbar. The panel must anchor to the notification area, not to
/// the flyout, and open fully on screen above the taskbar.
#[test]
fn test_icon_in_the_overflow_flyout_anchors_to_the_notification_area() {
    let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, TaskbarEdge::Bottom, 40);
    let flyout = rect(1600, 900, 140, 130);
    let icon = rect(1650, 950, 16, 16);
    let notify = rect(1640, 1040, 280, 40);
    let (anchor, source) =
        resolve_anchor(Some(icon), Some(flyout), Some(notify), monitor, work, 96);
    assert_eq!((anchor, source), (notify, AnchorSource::NotificationArea));
    let p = place_panel(&PlacementInput {
        icon: anchor,
        monitor,
        work,
        dpi: 96,
        logical_size: (420.0, 600.0),
    });
    assert_eq!(p.edge, TaskbarEdge::Bottom);
    assert_eq!(p.origin.1 + p.size.1 + p.gap, work.y + work.height);
    assert!(inside(p.origin, p.size, work));
}

/// An icon outside the flyout (pinned to the taskbar) is used as is; an
/// empty rectangle or none at all falls back to the notification area, then
/// to the taskbar corner.
#[test]
fn test_anchor_resolution_order() {
    let (monitor, work) = monitor_with_bar(0, 0, 1920, 1080, TaskbarEdge::Bottom, 40);
    let flyout = rect(1600, 900, 140, 130);
    let pinned = rect(1500, 1048, 24, 24);
    let notify = rect(1640, 1040, 280, 40);
    let resolve = |icon, overflow, area| resolve_anchor(icon, overflow, area, monitor, work, 96);
    assert_eq!(
        resolve(Some(pinned), Some(flyout), Some(notify)),
        (pinned, AnchorSource::Icon)
    );
    assert_eq!(
        resolve(Some(rect(0, 0, 0, 0)), None, Some(notify)),
        (notify, AnchorSource::NotificationArea)
    );
    assert_eq!(
        resolve(None, None, None),
        (fallback_icon(monitor, work, 96), AnchorSource::Fallback)
    );
    assert_eq!(
        resolve(None, None, Some(rect(0, 0, 0, 0))).1,
        AnchorSource::Fallback
    );
}
