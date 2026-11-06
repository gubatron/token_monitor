//! Windows event source: a hidden message-only window for `WM_POWERBROADCAST`
//! and a `NotifyNetworkConnectivityHintChange` subscription.
//!
//! The decoding and the gate logic are portable (see [`super::event`] and
//! [`super::machine`]); this file only moves OS messages into a
//! [`SystemEventSink`].

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, NotifyNetworkConnectivityHintChange,
};
use windows_sys::Win32::Networking::WinSock::NL_NETWORK_CONNECTIVITY_HINT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, HWND_MESSAGE, MSG, PostMessageW, PostQuitMessage, RegisterClassW,
    SetWindowLongPtrW, TranslateMessage, UnregisterClassW, WM_CLOSE, WM_DESTROY, WM_POWERBROADCAST,
    WNDCLASSW,
};

use super::event::{NetworkLevel, PowerEvent, ResumeDeduper};
use super::source::{SystemEventSink, SystemEventSource};

/// `TRUE` for `WM_POWERBROADCAST` (grants the request).
const TRUE_RESULT: LRESULT = 1;

/// Window class name, UTF-16 and NUL terminated: `TokenMonPowerWindow`.
fn class_name() -> Vec<u16> {
    "TokenMonPowerWindow\0".encode_utf16().collect()
}

/// State owned by the message thread and reachable from the window procedure.
struct WindowContext {
    sink: Arc<dyn SystemEventSink>,
    deduper: Mutex<ResumeDeduper>,
}

/// Delivers sleep, wake and connectivity changes from Windows to a sink.
///
/// Dropping the source closes the window, stops the thread and unsubscribes.
#[derive(Default)]
pub struct WindowsEventSource {
    hwnd: Arc<AtomicIsize>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl WindowsEventSource {
    /// A source that has not started.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SystemEventSource for WindowsEventSource {
    fn start(&self, sink: Arc<dyn SystemEventSink>) {
        let mut slot = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        let hwnd = Arc::clone(&self.hwnd);
        *slot = std::thread::Builder::new()
            .name("tokenmon-power".to_owned())
            .spawn(move || run_message_loop(sink, &hwnd))
            .ok();
    }
}

impl Drop for WindowsEventSource {
    fn drop(&mut self) {
        let hwnd = self.hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            // SAFETY: `hwnd` is the live message window created by the thread
            // we are about to join; posting WM_CLOSE is thread safe.
            unsafe { PostMessageW(hwnd as HWND, WM_CLOSE, 0, 0) };
        }
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = thread.join();
        }
    }
}

/// Window procedure: decodes `WM_POWERBROADCAST` and forwards it to the sink.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            // SAFETY: GWLP_USERDATA holds a `*const WindowContext` set before
            // the message loop starts and cleared only after it ends.
            let ctx = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WindowContext;
            // SAFETY: see above; the pointer is null or valid for the window's life.
            if let Some(ctx) = unsafe { ctx.as_ref() } {
                let event = ctx
                    .deduper
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .accept(wparam as u32);
                match event {
                    Some(PowerEvent::WillSleep) => ctx.sink.system_will_sleep(),
                    Some(PowerEvent::DidWake) => ctx.sink.system_did_wake(),
                    None => {}
                }
            }
            TRUE_RESULT
        }
        WM_CLOSE => {
            // SAFETY: `hwnd` is this window, destroyed on its own thread.
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            // SAFETY: ends the message loop of the current thread.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: default handling of a message we do not own.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Callback for connectivity hint changes; runs on a system thread.
unsafe extern "system" fn hint_changed(context: *const c_void, hint: NL_NETWORK_CONNECTIVITY_HINT) {
    // SAFETY: `context` is the `Arc<dyn SystemEventSink>` box leaked in
    // `subscribe_network` and freed only after the subscription is cancelled.
    let sink = unsafe { &*context.cast::<Arc<dyn SystemEventSink>>() };
    let level = NetworkLevel::from_raw(hint.ConnectivityLevel);
    sink.path_changed(level.is_satisfied());
}

/// Registers the window class, creates the window, subscribes to the network
/// and pumps messages until `WM_QUIT`.
fn run_message_loop(sink: Arc<dyn SystemEventSink>, published: &AtomicIsize) {
    let class = class_name();
    // SAFETY: a null module name yields the current executable's handle.
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    // SAFETY: `wc` and `class` outlive the call.
    if unsafe { RegisterClassW(&raw const wc) } == 0 {
        return;
    }
    // SAFETY: valid class, HWND_MESSAGE parent makes a message-only window.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        // SAFETY: the class was registered above on this instance.
        unsafe { UnregisterClassW(class.as_ptr(), instance) };
        return;
    }
    let ctx = Box::into_raw(Box::new(WindowContext {
        sink: Arc::clone(&sink),
        deduper: Mutex::new(ResumeDeduper::default()),
    }));
    // SAFETY: `ctx` stays valid until after the loop; freed below.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx as isize) };
    published.store(hwnd as isize, Ordering::Release);

    let (handle, leaked) = subscribe_network(sink);
    pump_messages();
    unsubscribe_network(handle, leaked);

    published.store(0, Ordering::Release);
    // SAFETY: the window is destroyed, so nothing can read `ctx` any more.
    drop(unsafe { Box::from_raw(ctx) });
    // SAFETY: the class was registered by this function.
    unsafe { UnregisterClassW(class.as_ptr(), instance) };
}

/// Subscribes to connectivity hints; also reports the initial level.
fn subscribe_network(sink: Arc<dyn SystemEventSink>) -> (HANDLE, *mut Arc<dyn SystemEventSink>) {
    let leaked = Box::into_raw(Box::new(sink));
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: `leaked` outlives the subscription; `handle` is a valid out pointer.
    let status = unsafe {
        NotifyNetworkConnectivityHintChange(
            Some(hint_changed),
            leaked.cast(),
            true,
            &raw mut handle,
        )
    };
    if status != 0 {
        handle = std::ptr::null_mut();
    }
    (handle, leaked)
}

/// Cancels the subscription (which waits for callbacks in flight) and frees the context.
fn unsubscribe_network(handle: HANDLE, leaked: *mut Arc<dyn SystemEventSink>) {
    if !handle.is_null() {
        // SAFETY: `handle` came from a successful subscription.
        unsafe { CancelMibChangeNotify2(handle) };
    }
    // SAFETY: no callback can run after the cancel above returned.
    drop(unsafe { Box::from_raw(leaked) });
}

/// Standard message pump until `WM_QUIT`.
fn pump_messages() {
    // SAFETY: MSG is plain data; zero is a valid initial value.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: `msg` is a valid out pointer; a null HWND reads this thread's queue.
    while unsafe { GetMessageW(&raw mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: `msg` was filled by GetMessageW.
        unsafe {
            TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}
