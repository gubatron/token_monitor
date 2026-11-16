//! `AppsUseLightTheme` and `DWMWA_USE_IMMERSIVE_DARK_MODE`: registry and DWM
//! calls only; the mapping is `tokenmon_core::AppTheme`.
#![allow(unsafe_code)]

use tokenmon_core::AppTheme;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

/// `DWMWA_USE_IMMERSIVE_DARK_MODE` (Windows 10 20H1 and later; earlier builds
/// ignore the call and keep a light title bar).
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn apps_use_light_theme() -> Option<u32> {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `key` and `name` are NUL-terminated UTF-16 buffers that outlive
    // the call; `value`/`size` are valid out pointers of the sizes declared.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &raw mut size,
        )
    };
    (status == 0).then_some(value)
}

/// The app theme right now (light when the value is missing).
pub fn apps_theme() -> AppTheme {
    AppTheme::from_apps_use_light_theme(apps_use_light_theme())
}

/// Switches the title bar of the top-level window `hwnd` between dark and
/// light. Failure (an old Windows build) is ignored: the title bar just stays
/// as it was.
pub fn set_dark_title_bar(hwnd: isize, theme: AppTheme) {
    let on = i32::from(theme.is_dark());
    // SAFETY: `hwnd` is a window handle of this process; the attribute value
    // is a 4-byte BOOL that outlives the call.
    unsafe {
        DwmSetWindowAttribute(
            hwnd as HWND,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const on).cast(),
            std::mem::size_of::<i32>() as u32,
        );
    }
}
