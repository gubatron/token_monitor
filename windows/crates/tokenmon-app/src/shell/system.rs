//! Small Win32 wrappers the shell needs: a startup-failure message box, the
//! foreground-rights hand-over for a second launch, and the hidden top-level
//! window that receives the session-end messages (`WM_QUERYENDSESSION` /
//! `WM_ENDSESSION`), which winit and Slint do not surface. The decisions are
//! in [`crate::session`] and [`crate::startup`].
#![allow(unsafe_code)]

use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, MB_ICONERROR, MB_OK,
    MB_SETFOREGROUND, MessageBoxW, RegisterClassW, WNDCLASSW,
};

use crate::session::classify;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shows a modal error box (the app has no console and no window yet).
pub fn error_box(title: &str, text: &str) {
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both buffers are NUL-terminated and outlive the call; a null
    // owner window is allowed.
    unsafe {
        MessageBoxW(
            null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// Lets any process take the foreground. A second launch calls this before it
/// hands "show the panel" to the running instance, which has no foreground
/// rights of its own (it was not the process the user just interacted with).
pub fn allow_any_process_foreground() {
    // SAFETY: a pure call taking an integer.
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

/// Runs when the session really ends. Set once by [`create_session_window`].
static ON_SESSION_END: OnceLock<fn()> = OnceLock::new();

unsafe extern "system" fn session_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(m) = classify(message, wparam) {
        if m.must_quit()
            && let Some(on_end) = ON_SESSION_END.get()
        {
            on_end();
        }
        return m.reply();
    }
    // SAFETY: forwarding the unchanged arguments of this window procedure.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Creates the hidden top-level window on the calling (UI) thread; the event
/// loop's message pump delivers its messages. `on_end` runs on this thread
/// inside the window procedure and must finish quickly (Windows allows a few
/// seconds before it kills the process).
///
/// # Errors
/// The window class or window could not be created.
pub fn create_session_window(on_end: fn()) -> Result<(), String> {
    let _ = ON_SESSION_END.set(on_end);
    let class = wide("TokenMon.SessionEnd");
    let title = wide("TokenMon");
    // SAFETY: `class` and `title` are NUL-terminated and live across the
    // calls; the class is zero-initialised with only the fields set below,
    // and the window procedure above has the required signature.
    unsafe {
        let instance = GetModuleHandleW(null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(session_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        if RegisterClassW(&raw const wc) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // Not message-only (HWND_MESSAGE windows do not get the session
        // broadcast) and never shown.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}
