//! Monitor facts the tray and panel need: the monitor and work area under a
//! point, its DPI, and the small-icon metric at that DPI. Thin wrappers over
//! Win32; the decisions on top of them are in [`crate::placement`] and
//! [`crate::tray`].
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

use crate::tray::Rect;

/// Fallback when the monitor cannot be queried: a common 1080p screen.
const FALLBACK_MONITOR: Rect = Rect {
    x: 0,
    y: 0,
    width: 1920,
    height: 1080,
};
const FALLBACK_WORK: Rect = Rect {
    x: 0,
    y: 0,
    width: 1920,
    height: 1040,
};

/// One monitor's geometry and scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorFacts {
    /// The whole monitor, virtual-screen physical pixels.
    pub monitor: Rect,
    /// The monitor minus the taskbar.
    pub work: Rect,
    /// Effective DPI (96 = 100%).
    pub dpi: u32,
}

/// A Win32 `RECT` as a [`Rect`].
pub fn rect_from(r: RECT) -> Rect {
    Rect {
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    }
}

/// The monitor nearest `(x, y)`.
pub fn monitor_at(x: i32, y: i32) -> MonitorFacts {
    // SAFETY: `MonitorFromPoint` takes a plain POINT by value and always
    // returns a monitor for `MONITOR_DEFAULTTONEAREST`; `info` is a properly
    // sized, zero-initialised MONITORINFO whose `cbSize` is set as required;
    // `dpi_x`/`dpi_y` are valid out pointers.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let (rect, work) = if GetMonitorInfoW(monitor, &raw mut info) == 0 {
            (FALLBACK_MONITOR, FALLBACK_WORK)
        } else {
            (rect_from(info.rcMonitor), rect_from(info.rcWork))
        };
        let (mut dpi_x, mut dpi_y) = (96_u32, 96_u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) != 0 {
            dpi_x = 96;
        }
        MonitorFacts {
            monitor: rect,
            work,
            dpi: dpi_x,
        }
    }
}

/// `SM_CXSMICON` at `dpi` (0 when unavailable).
pub fn small_icon_metric(dpi: u32) -> u32 {
    // SAFETY: a pure query taking two integers.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    u32::try_from(px).unwrap_or(0)
}
