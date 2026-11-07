//! The TokenMon icon on the sign-in window (Windows only): a thin adapter over
//! `LoadImageW` and `WM_SETICON`.
#![allow(unsafe_code)]

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_SHARED, LoadImageW, SM_CXICON,
    SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
};
use windows::core::PCWSTR;

/// Resource id of the executable's group icon (`build.rs` of the app crate).
const APP_ICON_ID: usize = 1;

/// Gives the window the TokenMon icon (big for Alt+Tab, small for the title
/// bar and taskbar button) from the exe's icon resource, so the sign-in
/// window never shows the generic application icon (#81). A test host or an
/// exe without the resource keeps the default.
///
/// # Safety
/// `hwnd` must be a live window of the calling thread.
pub(super) unsafe fn set_app_icon(hwnd: HWND, hinst: HINSTANCE) {
    for (kind, metric_w, metric_h) in [
        (ICON_BIG, SM_CXICON, SM_CYICON),
        (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
    ] {
        // SAFETY: loads a shared icon resource from our own module at the
        // metric size; the handle is owned by the system (LR_SHARED).
        unsafe {
            let icon = LoadImageW(
                Some(hinst),
                PCWSTR(APP_ICON_ID as *const u16),
                IMAGE_ICON,
                GetSystemMetrics(metric_w),
                GetSystemMetrics(metric_h),
                LR_SHARED,
            );
            if let Ok(icon) = icon {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
    }
}
