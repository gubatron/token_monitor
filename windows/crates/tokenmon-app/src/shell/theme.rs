//! Reads the taskbar theme from the registry
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`,
//! `SystemUsesLightTheme`).
#![allow(unsafe_code)]

use tokenmon_ui::tray_icon::TaskbarTheme;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The current `SystemUsesLightTheme` value, `None` when unreadable.
fn system_uses_light_theme() -> Option<u32> {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("SystemUsesLightTheme");
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

/// The taskbar theme right now (dark when the value is missing).
pub fn taskbar_theme() -> TaskbarTheme {
    TaskbarTheme::from_system_uses_light_theme(system_uses_light_theme())
}
