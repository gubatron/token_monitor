//! Monitor facts the tray and panel need: the work area under a point and the
//! small-icon metric at that monitor's DPI. Thin wrappers over Win32; the
//! decisions on top of them are in [`crate::tray`].
#![allow(unsafe_code)]

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::SM_CXSMICON;

use crate::tray::Rect;

/// Fallback when the monitor cannot be queried: a common 1080p work area.
const FALLBACK: Rect = Rect {
    x: 0,
    y: 0,
    width: 1920,
    height: 1040,
};

/// The monitor nearest `(x, y)`: its work area and its effective DPI.
pub fn monitor_at(x: i32, y: i32) -> (Rect, u32) {
    // SAFETY: `MonitorFromPoint` takes a plain POINT by value and always
    // returns a monitor for `MONITOR_DEFAULTTONEAREST`; `info` is a properly
    // sized, zero-initialised MONITORINFO whose `cbSize` is set as required;
    // `dpi_x`/`dpi_y` are valid out pointers.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work = if GetMonitorInfoW(monitor, &raw mut info) == 0 {
            FALLBACK
        } else {
            Rect {
                x: info.rcWork.left,
                y: info.rcWork.top,
                width: info.rcWork.right - info.rcWork.left,
                height: info.rcWork.bottom - info.rcWork.top,
            }
        };
        let (mut dpi_x, mut dpi_y) = (96_u32, 96_u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) != 0 {
            dpi_x = 96;
        }
        (work, dpi_x)
    }
}

/// `SM_CXSMICON` at `dpi` (0 when unavailable).
pub fn small_icon_metric(dpi: u32) -> u32 {
    // SAFETY: a pure query taking two integers.
    let px = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    u32::try_from(px).unwrap_or(0)
}
