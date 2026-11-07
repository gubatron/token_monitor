//! Small Win32 wrappers the shell needs: a startup-failure message box, the
//! foreground-rights hand-over for a second launch, and the hidden top-level
//! window that receives the session-end messages (`WM_QUERYENDSESSION` /
//! `WM_ENDSESSION`), which winit and Slint do not surface. The decisions are
//! in [`crate::session`] and [`crate::startup`].
#![allow(unsafe_code)]

use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, BringWindowToTop, CreateWindowExW, DefWindowProcW,
    FindWindowExW, FindWindowW, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, IsWindowVisible,
    MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW, RegisterClassW, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, WNDCLASSW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

use crate::session::classify;
use crate::tray::Rect;

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

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `hwnd` is non-null and `r` is a valid out pointer.
    let ok = !hwnd.is_null() && unsafe { GetWindowRect(hwnd, &raw mut r) } != 0;
    ok.then(|| super::display::rect_from(r))
}

/// The visible notification-area overflow flyout (the `^` popup), if open.
pub fn overflow_flyout_rect() -> Option<Rect> {
    let class = wide("NotifyIconOverflowWindow");
    // SAFETY: `class` is NUL-terminated and outlives the calls; a window
    // that dies concurrently makes the API report failure.
    let hwnd = unsafe { FindWindowW(class.as_ptr(), null()) };
    if hwnd.is_null() || unsafe { IsWindowVisible(hwnd) } == 0 {
        return None;
    }
    window_rect(hwnd)
}

/// The taskbar's notification area (`Shell_TrayWnd` > `TrayNotifyWnd`).
pub fn notification_area_rect() -> Option<Rect> {
    let (tray, notify) = (wide("Shell_TrayWnd"), wide("TrayNotifyWnd"));
    // SAFETY: both buffers are NUL-terminated and outlive the calls.
    let hwnd = unsafe {
        let taskbar = FindWindowW(tray.as_ptr(), null());
        if taskbar.is_null() {
            return None;
        }
        FindWindowExW(taskbar, null_mut(), notify.as_ptr(), null())
    };
    window_rect(hwnd)
}

/// Makes the window a tool window: no taskbar button and no Alt+Tab entry.
/// Deliberately not topmost, so other windows can cover the panel (#81).
pub fn make_tool_window(hwnd: isize) {
    let hwnd = hwnd as HWND;
    // SAFETY: `hwnd` is the live HWND of a window owned by this thread; the
    // calls only read and update its extended style.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let tool = isize::try_from(WS_EX_TOOLWINDOW).unwrap_or(0);
        let app = isize::try_from(WS_EX_APPWINDOW).unwrap_or(0);
        let wanted = (ex | tool) & !app;
        if wanted != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, wanted);
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

/// Brings the window to the foreground and gives it keyboard focus. Windows
/// allows this for the process that just handled the tray click. Returns
/// whether the system accepted it.
pub fn bring_to_front(hwnd: isize) -> bool {
    let hwnd = hwnd as HWND;
    // SAFETY: plain calls on the live HWND of this thread's window.
    unsafe {
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd) != 0
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
