//! The Windows adapter: a native Win32 window hosting WebView2.
//!
//! This file contains no sign-in decisions. It translates WebView2 and Win32
//! events into [`Input`]s for the portable [`SignInMachine`] and executes the
//! [`Action`]s it returns. The chrome (header, navigation bar, footer) is made
//! of native Win32 controls so no provider page ever shares a document or a
//! script context with it.
//!
//! One window per sign-in session runs on its own thread with its own STA and
//! message loop, so it never blocks, or is blocked by, the Slint event loop.
//!
//! Isolation and hardening applied to every web view (main and popups):
//!
//! * host objects and web messaging are off, DevTools, browser accelerator
//!   keys, the status bar and password autosave are off;
//! * navigation and `window.open` targets other than `http(s)` are refused;
//! * permission prompts are denied and downloads are cancelled;
//! * the user agent is never touched (Google refuses non-browser agents).
//!
//! The environment's user-data folder is `<root>/<provider>/<session>`; it is
//! deleted when the window closes and purged at startup, so no profile data
//! outlives a sign-in.

// FFI-heavy adapter: wildcard imports of the generated Win32/WebView2 bindings,
// `&mut x` out-pointers and short-lived single-letter handles are the idiom here.
#![allow(
    clippy::wildcard_imports,
    clippy::borrow_as_ptr,
    clippy::many_single_char_names,
    clippy::needless_pass_by_value,
    clippy::ref_as_ptr,
    clippy::cast_lossless,
    clippy::cast_possible_wrap,
    clippy::default_trait_access
)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, mpsc};
use std::time::Duration;

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, GetCookiesCompletedHandler, HistoryChangedEventHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, PermissionRequestedEventHandler,
    ServerCertificateErrorDetectedEventHandler, SourceChangedEventHandler,
    WindowCloseRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateFontW, DEFAULT_GUI_FONT, DeleteObject, GetStockObject, HBRUSH, HFONT,
    HGDIOBJ,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{
    SS_ENDELLIPSIS, SS_ETCHEDHORZ, SS_LEFT, SS_PATHELLIPSIS,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, Interface, PCWSTR, PWSTR, w};

use super::config::{SignInConfig, WINDOW_MIN_SIZE, WINDOW_SIZE};
use super::cookies::WebCookie;
use super::gate::NavigationPolicy;
use super::host::{SignInDelegate, WebAuthError, WebAuthHost};
use super::machine::{Action, Input, SignInMachine, TimerId, ViewKind, ViewModel};
use super::nav::NavSnapshot;
use super::profile::{ProfileStore, remove_dir_if_exists};

const ID_BACK: i32 = 101;
const ID_FORWARD: i32 = 102;
const ID_RELOAD: i32 = 103;
const ID_CLOSE_POPUP: i32 = 104;
const ID_FINISH: i32 = IDOK.0;
const ID_CANCEL: i32 = IDCANCEL.0;

const WM_APP_FOCUS: u32 = WM_APP + 1;
const WM_APP_SIGN_OUT: u32 = WM_APP + 2;

const HEADER_H: i32 = 72;
const NAV_H: i32 = 40;
const FOOTER_H: i32 = 64;

const TIMERS: [TimerId; 6] = [
    TimerId::StatusHint,
    TimerId::PopupPoll,
    TimerId::ReturnDelay,
    TimerId::CaptureAttempt,
    TimerId::AutoRetry,
    TimerId::ConfirmDismiss,
];

fn timer_native_id(id: TimerId) -> usize {
    TIMERS.iter().position(|t| *t == id).map_or(1, |i| i + 1)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn scaled(v: i32, dpi: u32) -> i32 {
    (v as i64 * i64::from(dpi) / 96) as i32
}

// ---------------------------------------------------------------------------
// Host: owns the windows, one thread each.
// ---------------------------------------------------------------------------

struct WindowHandle {
    hwnd: isize,
    done: Arc<AtomicBool>,
}

/// [`WebAuthHost`] backed by WebView2.
pub struct WindowsWebAuthHost {
    profiles: ProfileStore,
    windows: Mutex<HashMap<String, WindowHandle>>,
    counter: AtomicU64,
}

impl WindowsWebAuthHost {
    /// A host keeping profiles under `profile_root`
    /// (`%LOCALAPPDATA%\TokenMon\signin`). Leftovers of crashed sessions are
    /// purged immediately, so no profile survives across runs.
    pub fn new(profile_root: impl Into<std::path::PathBuf>) -> Self {
        let profiles = ProfileStore::new(profile_root);
        let _ = profiles.purge_all();
        Self {
            profiles,
            windows: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }
    }

    fn live_hwnd(&self, slug: &str) -> Option<isize> {
        let map = self.windows.lock().ok()?;
        let h = map.get(slug)?;
        (!h.done.load(Ordering::SeqCst)).then_some(h.hwnd)
    }
}

/// Whether the WebView2 runtime is installed.
pub fn webview2_available() -> bool {
    let mut version = PWSTR::null();
    // SAFETY: `version` is a valid out pointer; a null folder means the
    // installed Evergreen runtime. On success the CoTaskMem string is freed by
    // `take_pwstr`.
    unsafe {
        if GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version).is_ok() {
            drop(take_pwstr(version));
            return true;
        }
    }
    false
}

impl WebAuthHost for WindowsWebAuthHost {
    fn open(
        &self,
        config: SignInConfig,
        delegate: Arc<dyn SignInDelegate>,
    ) -> Result<(), WebAuthError> {
        if config.start_url == "about:blank"
            || !NavigationPolicy::from_config(&config).allows(&config.start_url)
        {
            return Err(WebAuthError::InvalidConfig(
                "start URL must be https on a provider or identity host".into(),
            ));
        }
        if let Some(raw) = self.live_hwnd(&config.provider_slug) {
            // SAFETY: PostMessageW is thread-safe and tolerates a dead HWND.
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut c_void)),
                    WM_APP_FOCUS,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            return Ok(());
        }
        if !webview2_available() {
            return Err(WebAuthError::BrowserUnavailable(
                "the Microsoft Edge WebView2 Runtime is not installed".into(),
            ));
        }
        let token = format!(
            "s{}-{}",
            std::process::id(),
            self.counter.fetch_add(1, Ordering::SeqCst)
        );
        let dir = self
            .profiles
            .session_dir(&config.provider_slug, &token)
            .map_err(|e| WebAuthError::InvalidConfig(e.to_string()))?;
        let slug = config.provider_slug.clone();
        let done = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<isize, WebAuthError>>();
        let thread_done = Arc::clone(&done);
        std::thread::Builder::new()
            .name(format!("signin-{slug}"))
            .spawn(move || {
                window_thread(config, delegate, dir.clone(), &tx);
                thread_done.store(true, Ordering::SeqCst);
                delete_profile(&dir);
            })
            .map_err(|e| WebAuthError::BrowserUnavailable(e.to_string()))?;
        let hwnd = rx
            .recv()
            .map_err(|_| WebAuthError::BrowserUnavailable("sign-in window thread died".into()))??;
        self.windows
            .lock()
            .map_err(|_| WebAuthError::BrowserUnavailable("lock poisoned".into()))?
            .insert(slug, WindowHandle { hwnd, done });
        Ok(())
    }

    fn sign_out(&self, provider_slug: &str) {
        if let Some(raw) = self.live_hwnd(provider_slug) {
            // SAFETY: see `open`.
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(raw as *mut c_void)),
                    WM_APP_SIGN_OUT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        // Folders of closed sessions are already gone; this catches leftovers
        // of this provider only (never another provider's).
        let _ = self.profiles.purge_provider(provider_slug);
    }
}

/// Deletes a session folder; the browser process can hold files for a moment
/// after the controller closes, so retry for a few seconds. Anything left is
/// removed by the startup purge.
fn delete_profile(dir: &std::path::Path) {
    for _ in 0..40 {
        if remove_dir_if_exists(dir).is_ok() {
            if let Some(parent) = dir.parent() {
                let _ = std::fs::remove_dir(parent); // only if now empty
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ---------------------------------------------------------------------------
// Window thread
// ---------------------------------------------------------------------------

struct Controls {
    title: HWND,
    status: HWND,
    cancel: HWND,
    back: HWND,
    forward: HWND,
    reload: HWND,
    address: HWND,
    close_popup: HWND,
    finish: HWND,
    seps: [HWND; 3],
}

struct View {
    id: u64,
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    loading: Rc<Cell<bool>>,
}

struct Ctx {
    hwnd: Cell<HWND>,
    machine: RefCell<SignInMachine>,
    queue: RefCell<VecDeque<Input>>,
    busy: Cell<bool>,
    delegate: Arc<dyn SignInDelegate>,
    start_url: String,
    nav: NavigationPolicy,
    env: RefCell<Option<ICoreWebView2Environment>>,
    cookie_manager: RefCell<Option<ICoreWebView2CookieManager>>,
    main: RefCell<Option<View>>,
    popups: RefCell<Vec<View>>,
    next_view_id: Cell<u64>,
    controls: RefCell<Option<Controls>>,
    fonts: RefCell<Vec<HFONT>>,
    closed_notified: Cell<bool>,
}

static REGISTER: Once = Once::new();
const CLASS_NAME: PCWSTR = w!("TokenMonSignInWindow");

fn window_thread(
    config: SignInConfig,
    delegate: Arc<dyn SignInDelegate>,
    user_data: std::path::PathBuf,
    tx: &mpsc::Sender<Result<isize, WebAuthError>>,
) {
    // SAFETY: balanced with CoUninitialize below, once per thread.
    let com_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    run_window(config, delegate, &user_data, tx);
    if com_ok {
        // SAFETY: matches the successful CoInitializeEx; all COM objects of
        // this thread were dropped inside `run_window`.
        unsafe { CoUninitialize() };
    }
}

fn run_window(
    config: SignInConfig,
    delegate: Arc<dyn SignInDelegate>,
    user_data: &std::path::Path,
    tx: &mpsc::Sender<Result<isize, WebAuthError>>,
) {
    let ctx = Rc::new(Ctx {
        hwnd: Cell::new(HWND::default()),
        machine: RefCell::new(SignInMachine::new(&config)),
        queue: RefCell::new(VecDeque::new()),
        busy: Cell::new(false),
        delegate,
        start_url: config.start_url.clone(),
        nav: NavigationPolicy::from_config(&config),
        env: RefCell::new(None),
        cookie_manager: RefCell::new(None),
        main: RefCell::new(None),
        popups: RefCell::new(Vec::new()),
        next_view_id: Cell::new(1),
        controls: RefCell::new(None),
        fonts: RefCell::new(Vec::new()),
        closed_notified: Cell::new(false),
    });
    let hwnd = match create_window(&ctx, &config.title) {
        Ok(h) => h,
        Err(e) => {
            let _ = tx.send(Err(WebAuthError::BrowserUnavailable(e.to_string())));
            return;
        }
    };
    ctx.hwnd.set(hwnd);
    let _ = tx.send(Ok(hwnd.0 as isize));
    if let Err(e) = create_controls(&ctx) {
        ctx.show_error(&format!("Could not build the window: {e}"));
    }
    ctx.layout();
    // SAFETY: `hwnd` is the live window created above.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    ctx.dispatch(Input::Opened);
    ctx.render();
    if let Err(e) = ctx.start_browser(user_data) {
        ctx.show_error(&format!("Could not start the embedded browser: {e}"));
    }
    message_loop(hwnd);
    ctx.shutdown();
}

fn message_loop(hwnd: HWND) {
    let mut msg = MSG::default();
    // SAFETY: standard message loop on the thread that owns `hwnd`.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn create_window(ctx: &Rc<Ctx>, title: &str) -> windows::core::Result<HWND> {
    // SAFETY: Win32 window creation on the calling thread; `param` is a
    // leaked `Rc<Ctx>` clone reclaimed in WM_NCDESTROY.
    unsafe {
        let hinst = HINSTANCE(GetModuleHandleW(None)?.0);
        REGISTER.call_once(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                lpszClassName: CLASS_NAME,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut c_void),
                ..Default::default()
            };
            RegisterClassW(&class);
        });
        let dpi = GetDpiForSystem();
        let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: scaled(WINDOW_SIZE.0 as i32, dpi),
            bottom: scaled(WINDOW_SIZE.1 as i32, dpi),
        };
        let _ = AdjustWindowRect(&mut rect, style, false);
        let title = wide(title);
        let param = Box::into_raw(Box::new(Rc::clone(ctx))) as *const c_void;
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(title.as_ptr()),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(hinst),
            Some(param),
        )
    }
}

fn ctx_from(hwnd: HWND) -> Option<Rc<Ctx>> {
    // SAFETY: GWLP_USERDATA holds a leaked `Box<Rc<Ctx>>` set in WM_NCCREATE
    // and cleared in WM_NCDESTROY; only this thread touches it.
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Rc<Ctx>;
        p.as_ref().cloned()
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by the system with a valid window on its owning thread.
    unsafe {
        match msg {
            WM_NCCREATE => {
                let cs = lparam.0 as *const CREATESTRUCTW;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
            }
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Rc<Ctx>;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
            }
            WM_SIZE => {
                if let Some(ctx) = ctx_from(hwnd) {
                    ctx.layout();
                }
            }
            WM_GETMINMAXINFO => {
                let info = lparam.0 as *mut MINMAXINFO;
                let dpi = GetDpiForWindow(hwnd).max(96);
                let mut r = RECT {
                    left: 0,
                    top: 0,
                    right: scaled(WINDOW_MIN_SIZE.0 as i32, dpi),
                    bottom: scaled(WINDOW_MIN_SIZE.1 as i32, dpi),
                };
                let _ = AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false);
                (*info).ptMinTrackSize.x = r.right - r.left;
                (*info).ptMinTrackSize.y = r.bottom - r.top;
                return LRESULT(0);
            }
            WM_COMMAND => {
                if let Some(ctx) = ctx_from(hwnd) {
                    ctx.on_command((wparam.0 & 0xffff) as i32);
                }
                return LRESULT(0);
            }
            WM_TIMER => {
                let _ = KillTimer(Some(hwnd), wparam.0);
                if let (Some(ctx), Some(id)) = (
                    ctx_from(hwnd),
                    wparam.0.checked_sub(1).and_then(|i| TIMERS.get(i)),
                ) {
                    ctx.dispatch(Input::TimerFired(*id));
                }
                return LRESULT(0);
            }
            WM_APP_FOCUS => {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
                return LRESULT(0);
            }
            WM_APP_SIGN_OUT => {
                if let Some(ctx) = ctx_from(hwnd) {
                    ctx.delete_all_cookies();
                }
                return LRESULT(0);
            }
            WM_DESTROY => {
                if let Some(ctx) = ctx_from(hwnd) {
                    ctx.on_destroyed();
                }
                PostQuitMessage(0);
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

fn create_controls(ctx: &Rc<Ctx>) -> windows::core::Result<()> {
    let parent = ctx.hwnd.get();
    // SAFETY: child control creation on the window's thread; handles stay
    // valid until the parent is destroyed.
    unsafe {
        let hinst = HINSTANCE(GetModuleHandleW(None)?.0);
        let dpi = GetDpiForWindow(parent).max(96);
        let make = |class: PCWSTR,
                    text: &str,
                    style: WINDOW_STYLE,
                    id: i32|
         -> windows::core::Result<HWND> {
            let t = wide(text);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR(t.as_ptr()),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                10,
                10,
                Some(parent),
                Some(HMENU(id as usize as *mut c_void)),
                Some(hinst),
                None,
            )
        };
        let button = WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32);
        let default_button = WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32);
        let label = WINDOW_STYLE(SS_LEFT.0) | WINDOW_STYLE(SS_ENDELLIPSIS.0);
        let sep = WINDOW_STYLE(SS_ETCHEDHORZ.0);
        let controls = Controls {
            title: make(w!("STATIC"), "", label, 0)?,
            status: make(w!("STATIC"), "", WINDOW_STYLE(SS_LEFT.0), 0)?,
            cancel: make(w!("BUTTON"), "Cancel", button, ID_CANCEL)?,
            back: make(w!("BUTTON"), "\u{2190} Back", button, ID_BACK)?,
            forward: make(w!("BUTTON"), "Forward \u{2192}", button, ID_FORWARD)?,
            reload: make(w!("BUTTON"), "Reload", button, ID_RELOAD)?,
            address: make(
                w!("STATIC"),
                "",
                WINDOW_STYLE(SS_LEFT.0) | WINDOW_STYLE(SS_PATHELLIPSIS.0),
                0,
            )?,
            close_popup: make(w!("BUTTON"), "Close popup", button, ID_CLOSE_POPUP)?,
            finish: make(w!("BUTTON"), "Finish Sign-In", default_button, ID_FINISH)?,
            seps: [
                make(w!("STATIC"), "", sep, 0)?,
                make(w!("STATIC"), "", sep, 0)?,
                make(w!("STATIC"), "", sep, 0)?,
            ],
        };
        let normal = HFONT(GetStockObject(DEFAULT_GUI_FONT).0);
        let heading = CreateFontW(
            -scaled(18, dpi),
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            0,
            w!("Segoe UI"),
        );
        let set_font = |h: HWND, f: HFONT| {
            SendMessageW(h, WM_SETFONT, Some(WPARAM(f.0 as usize)), Some(LPARAM(1)));
        };
        for h in [
            controls.status,
            controls.cancel,
            controls.back,
            controls.forward,
            controls.reload,
            controls.address,
            controls.close_popup,
            controls.finish,
        ] {
            set_font(h, normal);
        }
        set_font(
            controls.title,
            if heading.is_invalid() {
                normal
            } else {
                heading
            },
        );
        if !heading.is_invalid() {
            ctx.fonts.borrow_mut().push(heading);
        }
        *ctx.controls.borrow_mut() = Some(controls);
    }
    Ok(())
}

fn body_rect(client: RECT, dpi: u32) -> RECT {
    RECT {
        left: 0,
        top: scaled(HEADER_H + NAV_H + 2, dpi),
        right: client.right,
        bottom: (client.bottom - scaled(FOOTER_H + 1, dpi)).max(scaled(HEADER_H + NAV_H + 2, dpi)),
    }
}

impl Ctx {
    fn client_and_dpi(&self) -> Option<(RECT, u32)> {
        let hwnd = self.hwnd.get();
        let mut r = RECT::default();
        // SAFETY: live window of this thread.
        unsafe {
            GetClientRect(hwnd, &mut r).ok()?;
            Some((r, GetDpiForWindow(hwnd).max(96)))
        }
    }

    fn layout(&self) {
        let Some((c, dpi)) = self.client_and_dpi() else {
            return;
        };
        let controls = self.controls.borrow();
        let Some(k) = controls.as_ref() else { return };
        let s = |v: i32| scaled(v, dpi);
        let w = c.right;
        let h = c.bottom;
        let mv = |hwnd: HWND, x: i32, y: i32, cw: i32, ch: i32| {
            // SAFETY: child window of this thread.
            unsafe {
                let _ = MoveWindow(hwnd, x, y, cw.max(0), ch.max(0), true);
            }
        };
        mv(k.title, s(16), s(10), w - s(16 + 16 + 110), s(26));
        mv(k.status, s(16), s(38), w - s(16 + 16 + 110), s(30));
        mv(k.cancel, w - s(16 + 90), s(14), s(90), s(28));
        mv(k.seps[0], 0, s(HEADER_H), w, 2);
        let ny = s(HEADER_H + 1 + 6);
        mv(k.back, s(16), ny, s(80), s(28));
        mv(k.forward, s(102), ny, s(96), s(28));
        mv(k.reload, s(204), ny, s(80), s(28));
        mv(k.address, s(296), ny + s(6), w - s(296 + 16 + 110), s(20));
        mv(k.close_popup, w - s(16 + 100), ny, s(100), s(28));
        mv(k.seps[1], 0, s(HEADER_H + NAV_H + 1), w, 2);
        mv(k.seps[2], 0, h - s(FOOTER_H), w, 2);
        mv(
            k.finish,
            w - s(16 + 150),
            h - s(FOOTER_H) + s(16),
            s(150),
            s(32),
        );
        drop(controls);
        let b = body_rect(c, dpi);
        for v in self.all_views() {
            // SAFETY: live controller of this thread.
            unsafe {
                let _ = v.SetBounds(b);
            }
        }
    }

    fn all_views(&self) -> Vec<ICoreWebView2Controller> {
        let mut out = Vec::new();
        if let Some(m) = self.main.borrow().as_ref() {
            out.push(m.controller.clone());
        }
        out.extend(self.popups.borrow().iter().map(|p| p.controller.clone()));
        out
    }

    fn render(&self) {
        let vm = self.machine.borrow().view_model();
        self.apply_view(&vm);
    }

    fn apply_view(&self, vm: &ViewModel) {
        let controls = self.controls.borrow();
        let Some(k) = controls.as_ref() else { return };
        let set = |h: HWND, t: &str| {
            let t = wide(t);
            // SAFETY: child window of this thread; `t` outlives the call.
            unsafe {
                let _ = SetWindowTextW(h, PCWSTR(t.as_ptr()));
            }
        };
        let title = if vm.show_check {
            format!("\u{2713} {}", vm.title)
        } else {
            vm.title.clone()
        };
        set(k.title, &title);
        set(k.status, &vm.status_text);
        set(k.address, &vm.address);
        set(k.reload, if vm.is_loading { "Stop" } else { "Reload" });
        set(k.finish, vm.primary_title);
        // SAFETY: child windows of this thread.
        unsafe {
            let _ = EnableWindow(k.back, vm.back_enabled);
            let _ = EnableWindow(k.forward, vm.forward_enabled);
            let _ = EnableWindow(k.finish, vm.primary_enabled);
            let _ = ShowWindow(
                k.close_popup,
                if vm.show_close_popup {
                    SW_SHOW
                } else {
                    SW_HIDE
                },
            );
        }
    }

    fn show_error(&self, message: &str) {
        if let Some(k) = self.controls.borrow().as_ref() {
            let t = wide(message);
            // SAFETY: child window of this thread.
            unsafe {
                let _ = SetWindowTextW(k.status, PCWSTR(t.as_ptr()));
            }
        }
    }

    // --- event plumbing ---------------------------------------------------

    fn dispatch(self: &Rc<Self>, input: Input) {
        self.queue.borrow_mut().push_back(input);
        if self.busy.replace(true) {
            return;
        }
        loop {
            let next = self.queue.borrow_mut().pop_front();
            let Some(i) = next else { break };
            let actions = self.machine.borrow_mut().handle(i);
            for a in actions {
                self.execute(a);
            }
        }
        self.busy.set(false);
    }

    fn execute(self: &Rc<Self>, action: Action) {
        let hwnd = self.hwnd.get();
        match action {
            Action::StartTimer { id, delay } => {
                let ms = u32::try_from(delay.as_millis()).unwrap_or(u32::MAX);
                // SAFETY: timer owned by our window.
                unsafe {
                    let _ = SetTimer(Some(hwnd), timer_native_id(id), ms, None);
                }
            }
            Action::CancelTimer(id) => {
                // SAFETY: see above.
                unsafe {
                    let _ = KillTimer(Some(hwnd), timer_native_id(id));
                }
            }
            Action::GoBack => {
                if let Some(wv) = self.active_webview() {
                    // SAFETY: live web view of this thread.
                    unsafe {
                        let _ = wv.GoBack();
                    }
                }
            }
            Action::ClosePopup => self.close_top_popup(),
            Action::ReadCookies => self.read_cookies(),
            Action::Adopt(captured) => {
                let result = self.delegate.adopt(captured);
                self.dispatch(Input::AdoptFinished(result));
            }
            Action::OnReturned(url) => self.delegate.on_returned(&url),
            Action::NotifyComplete => self.delegate.on_complete(),
            Action::CloseWindow => {
                // SAFETY: our window; WM_DESTROY re-enters `dispatch`, which
                // queues because `busy` is set.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            Action::UpdateView(vm) => self.apply_view(&vm),
        }
    }

    fn on_command(self: &Rc<Self>, id: i32) {
        match id {
            ID_BACK => self.dispatch(Input::BackClicked),
            ID_CLOSE_POPUP => self.dispatch(Input::ClosePopupClicked),
            ID_FINISH => self.dispatch(Input::FinishClicked),
            ID_CANCEL => self.dispatch(Input::CancelClicked),
            ID_FORWARD => {
                if let Some(wv) = self.active_webview() {
                    // SAFETY: live web view of this thread.
                    unsafe {
                        let _ = wv.GoForward();
                    }
                }
            }
            ID_RELOAD => {
                let loading = self.machine.borrow().nav().snapshot.is_loading;
                if let Some(wv) = self.active_webview() {
                    // SAFETY: live web view of this thread.
                    unsafe {
                        let _ = if loading { wv.Stop() } else { wv.Reload() };
                    }
                }
            }
            _ => {}
        }
    }

    fn on_destroyed(self: &Rc<Self>) {
        if !self.closed_notified.replace(true) {
            self.dispatch(Input::WindowClosed);
            self.delegate.on_closed();
        }
    }

    // --- web views --------------------------------------------------------

    fn active_webview(&self) -> Option<ICoreWebView2> {
        if let Some(p) = self.popups.borrow().last() {
            return Some(p.webview.clone());
        }
        self.main.borrow().as_ref().map(|m| m.webview.clone())
    }

    fn publish_snapshot(self: &Rc<Self>) {
        let (webview, loading) = if let Some(p) = self.popups.borrow().last() {
            (p.webview.clone(), p.loading.get())
        } else if let Some(m) = self.main.borrow().as_ref() {
            (m.webview.clone(), m.loading.get())
        } else {
            return;
        };
        let mut snap = NavSnapshot {
            is_loading: loading,
            ..NavSnapshot::default()
        };
        // SAFETY: live web view of this thread; out pointers are valid.
        unsafe {
            let mut b = BOOL(0);
            if webview.CanGoBack(&mut b).is_ok() {
                snap.can_go_back = b.as_bool();
            }
            if webview.CanGoForward(&mut b).is_ok() {
                snap.can_go_forward = b.as_bool();
            }
            let mut src = PWSTR::null();
            if webview.Source(&mut src).is_ok() {
                snap.current_url = Some(take_pwstr(src));
            }
        }
        self.dispatch(Input::NavSnapshot(snap));
    }

    fn start_browser(self: &Rc<Self>, user_data: &std::path::Path) -> windows::core::Result<()> {
        std::fs::create_dir_all(user_data)
            .map_err(|e| windows::core::Error::new(E_POINTER, e.to_string()))?;
        let udf = wide(&user_data.to_string_lossy());
        let (etx, erx) = mpsc::channel();
        CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                // SAFETY: `udf` outlives the call; null browser folder selects
                // the installed Evergreen runtime.
                unsafe {
                    CreateCoreWebView2EnvironmentWithOptions(
                        PCWSTR::null(),
                        PCWSTR(udf.as_ptr()),
                        None::<&ICoreWebView2EnvironmentOptions>,
                        &handler,
                    )
                    .map_err(webview2_com::Error::WindowsError)
                }
            }),
            Box::new(move |hr, env| {
                hr?;
                let _ = etx.send(env.ok_or_else(|| windows::core::Error::from(E_POINTER)));
                Ok(())
            }),
        )
        .map_err(|e| windows::core::Error::new(E_POINTER, format!("{e:?}")))?;
        let env = erx
            .recv()
            .map_err(|_| windows::core::Error::from(E_POINTER))??;
        *self.env.borrow_mut() = Some(env.clone());

        let (ctx_tx, ctx_rx) = mpsc::channel();
        let parent = self.hwnd.get();
        CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                // SAFETY: `parent` is our live window.
                unsafe {
                    env.CreateCoreWebView2Controller(parent, &handler)
                        .map_err(webview2_com::Error::WindowsError)
                }
            }),
            Box::new(move |hr, controller| {
                hr?;
                let _ =
                    ctx_tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)));
                Ok(())
            }),
        )
        .map_err(|e| windows::core::Error::new(E_POINTER, format!("{e:?}")))?;
        let controller = ctx_rx
            .recv()
            .map_err(|_| windows::core::Error::from(E_POINTER))??;
        let view = self.adopt_view(controller, ViewKind::Main)?;
        // SAFETY: live web view; the cast yields the profile's cookie manager.
        unsafe {
            let wv2 = view.webview.cast::<ICoreWebView2_2>()?;
            *self.cookie_manager.borrow_mut() = Some(wv2.CookieManager()?);
        }
        let webview = view.webview.clone();
        *self.main.borrow_mut() = Some(view);
        self.layout();
        let url = wide(&self.start_url);
        // SAFETY: `url` outlives the call.
        unsafe {
            webview.Navigate(PCWSTR(url.as_ptr()))?;
        }
        Ok(())
    }

    /// Hardens a new controller's web view and wires its events.
    fn adopt_view(
        self: &Rc<Self>,
        controller: ICoreWebView2Controller,
        kind: ViewKind,
    ) -> windows::core::Result<View> {
        let id = self.next_view_id.get();
        self.next_view_id.set(id + 1);
        // SAFETY: freshly created, live controller on this thread.
        let webview = unsafe { controller.CoreWebView2()? };
        let loading = Rc::new(Cell::new(false));
        // SAFETY: settings object belongs to the live web view.
        unsafe {
            let settings = webview.Settings()?;
            settings.SetAreHostObjectsAllowed(false)?;
            settings.SetIsWebMessageEnabled(false)?;
            settings.SetAreDevToolsEnabled(false)?;
            if let Ok(s3) = settings.cast::<ICoreWebView2Settings3>() {
                s3.SetAreBrowserAcceleratorKeysEnabled(false)?;
            }
            settings.SetIsStatusBarEnabled(false)?;
            if let Ok(s4) = settings.cast::<ICoreWebView2Settings4>() {
                let _ = s4.SetIsPasswordAutosaveEnabled(false);
                let _ = s4.SetIsGeneralAutofillEnabled(false);
            }
        }
        if let Some((c, dpi)) = self.client_and_dpi() {
            // SAFETY: live controller.
            unsafe {
                controller.SetBounds(body_rect(c, dpi))?;
                controller.SetIsVisible(true)?;
            }
        }
        self.wire_events(&webview, id, kind, &loading)?;
        Ok(View {
            id,
            controller,
            webview,
            loading,
        })
    }

    fn wire_events(
        self: &Rc<Self>,
        webview: &ICoreWebView2,
        id: u64,
        kind: ViewKind,
        loading: &Rc<Cell<bool>>,
    ) -> windows::core::Result<()> {
        let mut token = 0i64;
        let weak: Weak<Ctx> = Rc::downgrade(self);

        // Only https on the provider's own or identity hosts may be loaded;
        // everything else (other hosts, http, other schemes) is cancelled.
        let w = weak.clone();
        let l = Rc::clone(loading);
        let starting = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                // SAFETY: live event args for the duration of the callback.
                unsafe {
                    let mut uri = PWSTR::null();
                    args.Uri(&mut uri)?;
                    let uri = take_pwstr(uri);
                    match w.upgrade() {
                        Some(ctx) if ctx.nav.allows(&uri) => {
                            l.set(true);
                            ctx.publish_snapshot();
                        }
                        _ => args.SetCancel(true)?,
                    }
                }
            }
            Ok(())
        }));
        let w = weak.clone();
        let l = Rc::clone(loading);
        let completed = NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
            l.set(false);
            let Some(ctx) = w.upgrade() else {
                return Ok(());
            };
            // SAFETY: live sender and args for the duration of the callback.
            unsafe {
                let ok = args
                    .as_ref()
                    .and_then(|a| {
                        let mut b = BOOL(0);
                        a.IsSuccess(&mut b).ok().map(|()| b.as_bool())
                    })
                    .unwrap_or(false);
                if ok && let Some(wv) = sender {
                    let mut src = PWSTR::null();
                    if wv.Source(&mut src).is_ok() {
                        ctx.dispatch(Input::NavigationFinished {
                            view: kind,
                            url: take_pwstr(src),
                        });
                    }
                }
            }
            ctx.publish_snapshot();
            Ok(())
        }));
        let w = weak.clone();
        let source_changed = SourceChangedEventHandler::create(Box::new(move |_, _| {
            if let Some(ctx) = w.upgrade() {
                ctx.publish_snapshot();
            }
            Ok(())
        }));
        let w = weak.clone();
        let history_changed = HistoryChangedEventHandler::create(Box::new(move |_, _| {
            if let Some(ctx) = w.upgrade() {
                ctx.publish_snapshot();
            }
            Ok(())
        }));
        let w = weak.clone();
        let new_window = NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
            if let (Some(ctx), Some(args)) = (w.upgrade(), args) {
                ctx.on_new_window(&args)?;
            }
            Ok(())
        }));
        let w = weak.clone();
        let close_requested = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
            if kind == ViewKind::Popup
                && let Some(ctx) = w.upgrade()
            {
                ctx.close_popup_by_id(id);
            }
            Ok(())
        }));
        let permission = PermissionRequestedEventHandler::create(Box::new(|_, args| {
            if let Some(args) = args {
                // SAFETY: live event args.
                unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
            }
            Ok(())
        }));
        // SAFETY: live web view; handlers are COM objects owned by it.
        unsafe {
            webview.add_NavigationStarting(&starting, &mut token)?;
            webview.add_NavigationCompleted(&completed, &mut token)?;
            webview.add_SourceChanged(&source_changed, &mut token)?;
            webview.add_HistoryChanged(&history_changed, &mut token)?;
            webview.add_NewWindowRequested(&new_window, &mut token)?;
            webview.add_WindowCloseRequested(&close_requested, &mut token)?;
            webview.add_PermissionRequested(&permission, &mut token)?;
            // A credential-capture window never offers "continue anyway" on a
            // certificate error: cancel, which leaves the interstitial error
            // page without a bypass.
            if let Ok(wv14) = webview.cast::<ICoreWebView2_14>() {
                let cert =
                    ServerCertificateErrorDetectedEventHandler::create(Box::new(|_, args| {
                        if let Some(args) = args {
                            // SAFETY: live event args (inside the enclosing unsafe block).
                            args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL)?;
                        }
                        Ok(())
                    }));
                wv14.add_ServerCertificateErrorDetected(&cert, &mut token)?;
            }
            if let Ok(wv4) = webview.cast::<ICoreWebView2_4>() {
                let download = DownloadStartingEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        // SAFETY: live event args (inside the enclosing unsafe block).
                        args.SetCancel(true)?;
                    }
                    Ok(())
                }));
                wv4.add_DownloadStarting(&download, &mut token)?;
            }
        }
        Ok(())
    }

    /// `window.open` / target=_blank: a child web view in the same profile.
    fn on_new_window(
        self: &Rc<Self>,
        args: &ICoreWebView2NewWindowRequestedEventArgs,
    ) -> windows::core::Result<()> {
        // SAFETY: live event args for the duration of the callback.
        unsafe {
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            if !self.nav.allows(&take_pwstr(uri)) {
                return args.SetHandled(true);
            }
            let Some(env) = self.env.borrow().clone() else {
                return args.SetHandled(true);
            };
            let deferral = args.GetDeferral()?;
            let args = args.clone();
            let weak = Rc::downgrade(self);
            let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                move |hr, controller| {
                    let result = (|| -> windows::core::Result<()> {
                        hr?;
                        let ctx = weak
                            .upgrade()
                            .ok_or_else(|| windows::core::Error::from(E_POINTER))?;
                        let controller =
                            controller.ok_or_else(|| windows::core::Error::from(E_POINTER))?;
                        let view = ctx.adopt_view(controller, ViewKind::Popup)?;
                        args.SetNewWindow(&view.webview)?;
                        args.SetHandled(true)?;
                        ctx.push_popup(view);
                        Ok(())
                    })();
                    if result.is_err() {
                        let _ = args.SetHandled(true);
                    }
                    deferral.Complete()?;
                    Ok(())
                },
            ));
            env.CreateCoreWebView2Controller(self.hwnd.get(), &handler)
        }
    }

    fn push_popup(self: &Rc<Self>, view: View) {
        // The popup covers the view below it.
        if let Some(below) = self
            .popups
            .borrow()
            .last()
            .map(|p| p.controller.clone())
            .or_else(|| self.main.borrow().as_ref().map(|m| m.controller.clone()))
        {
            // SAFETY: live controller.
            unsafe {
                let _ = below.SetIsVisible(false);
            }
        }
        let controller = view.controller.clone();
        self.popups.borrow_mut().push(view);
        // SAFETY: live controller.
        unsafe {
            let _ = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
        }
        self.dispatch(Input::PopupOpened);
        self.publish_snapshot();
    }

    fn close_top_popup(self: &Rc<Self>) {
        let id = self.popups.borrow().last().map(|p| p.id);
        if let Some(id) = id {
            self.close_popup_by_id(id);
        }
    }

    fn close_popup_by_id(self: &Rc<Self>, id: u64) {
        let view = {
            let mut popups = self.popups.borrow_mut();
            let Some(pos) = popups.iter().position(|p| p.id == id) else {
                return;
            };
            popups.remove(pos)
        };
        // SAFETY: live controller; Close detaches and releases the popup.
        unsafe {
            let _ = view.webview.Stop();
            let _ = view.controller.Close();
        }
        drop(view);
        let below = self
            .popups
            .borrow()
            .last()
            .map(|p| p.controller.clone())
            .or_else(|| self.main.borrow().as_ref().map(|m| m.controller.clone()));
        if let Some(c) = below {
            // SAFETY: live controller.
            unsafe {
                let _ = c.SetIsVisible(true);
                let _ = c.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        }
        self.dispatch(Input::PopupClosed);
        self.publish_snapshot();
    }

    // --- cookies ----------------------------------------------------------

    fn read_cookies(self: &Rc<Self>) {
        let Some(manager) = self.cookie_manager.borrow().clone() else {
            self.dispatch(Input::CookiesRead(Vec::new()));
            return;
        };
        let weak = Rc::downgrade(self);
        let handler = GetCookiesCompletedHandler::create(Box::new(move |hr, list| {
            let cookies = match (hr, list) {
                (Ok(()), Some(list)) => convert_cookies(&list),
                _ => Vec::new(),
            };
            if let Some(ctx) = weak.upgrade() {
                ctx.dispatch(Input::CookiesRead(cookies));
            }
            Ok(())
        }));
        // SAFETY: live cookie manager; an empty URI returns every cookie of
        // this (isolated) profile, HttpOnly ones included.
        let started = unsafe { manager.GetCookies(w!(""), &handler) };
        if started.is_err() {
            self.dispatch(Input::CookiesRead(Vec::new()));
        }
    }

    /// Scoped sign-out for an open window: the profile is private to this
    /// provider, so clearing it cannot touch another provider's cookies.
    fn delete_all_cookies(&self) {
        if let Some(m) = self.cookie_manager.borrow().as_ref() {
            // SAFETY: live cookie manager.
            unsafe {
                let _ = m.DeleteAllCookies();
            }
        }
    }

    fn shutdown(&self) {
        for v in self.popups.borrow_mut().drain(..) {
            // SAFETY: live controller.
            unsafe {
                let _ = v.controller.Close();
            }
        }
        if let Some(m) = self.main.borrow_mut().take() {
            // SAFETY: live controller.
            unsafe {
                let _ = m.controller.Close();
            }
        }
        self.cookie_manager.borrow_mut().take();
        self.env.borrow_mut().take();
        for f in self.fonts.borrow_mut().drain(..) {
            // SAFETY: font created by this window; no longer selected anywhere.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(f.0));
            }
        }
    }
}

fn convert_cookies(list: &ICoreWebView2CookieList) -> Vec<WebCookie> {
    let mut out = Vec::new();
    // SAFETY: live cookie list; every out pointer is valid and CoTaskMem
    // strings are freed by `take_pwstr`.
    unsafe {
        let mut count = 0u32;
        if list.Count(&mut count).is_err() {
            return out;
        }
        for i in 0..count {
            let Ok(c) = list.GetValueAtIndex(i) else {
                continue;
            };
            let text = |f: &dyn Fn(*mut PWSTR) -> windows::core::Result<()>| {
                let mut p = PWSTR::null();
                f(&mut p).ok().map(|()| take_pwstr(p))
            };
            let (Some(name), Some(value), Some(domain), Some(path)) = (
                text(&|p| c.Name(p)),
                text(&|p| c.Value(p)),
                text(&|p| c.Domain(p)),
                text(&|p| c.Path(p)),
            ) else {
                continue;
            };
            let flag = |f: &dyn Fn(*mut BOOL) -> windows::core::Result<()>| {
                let mut b = BOOL(0);
                f(&mut b).is_ok() && b.as_bool()
            };
            let is_session = flag(&|b| c.IsSession(b));
            let mut expires = 0f64;
            let _ = c.Expires(&mut expires);
            out.push(WebCookie {
                name,
                value,
                domain,
                path,
                http_only: flag(&|b| c.IsHttpOnly(b)),
                secure: flag(&|b| c.IsSecure(b)),
                expires_unix: (!is_session).then_some(expires),
            });
        }
    }
    out
}
