//! Windows glue: single-instance startup, the tray icon, the panel window and
//! the platform collaborators. Everything decidable lives in the portable
//! modules ([`crate::panel`], [`crate::tray`], [`crate::overview`],
//! [`crate::settings_host`]); this one only connects them to the OS.
//!
//! Threads: a tokio runtime runs polling and IO; the Slint event loop runs on
//! the main thread and owns the tray, the panel window and Settings.
//! Background callbacks (pipe server, toast click, Settings quit) reach the UI
//! through `slint::invoke_from_event_loop`.

use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use slint::ComponentHandle;
use tokenmon_core::alerts::{Notifier, ThresholdAlert};
use tokenmon_core::polling::TokioSleeper;
use tokenmon_core::storage::AppPaths;
use tokenmon_core::update::UrlOpener;
use tokenmon_core::usage::Calendar;
use tokenmon_core::{Clock, ProviderId, SystemClock};
use tokenmon_platform::install::handle_startup_args;
use tokenmon_platform::notify::windows::WinRtToastBackend;
use tokenmon_platform::notify::{AumidRegistration, ToastNotifier};
use tokenmon_platform::power::{SystemEventSource, SystemWakeGate, windows::WindowsEventSource};
use tokenmon_platform::secrets::DpapiEncryptor;
use tokenmon_platform::secrets::register_owner_only_permissions;
use tokenmon_platform::shell::windows::{
    MutexInstanceLock, PipeClient, PipeServer, WinRegistry, current_user_pipe_name,
};
use tokenmon_platform::shell::{
    DesktopVerb, IpcMessage, LaunchArgs, LoginItemService, RunKeyLoginItem, StartupDecision,
    decide_startup,
};
use tokenmon_platform::webauth::WindowsWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;
use tokenmon_ui::panels::events::{PanelEvent, PanelHandler};
use tokenmon_ui::panels::overview::OverviewCardVm;
use tokenmon_ui::panels::switcher::{RowVm, build_switcher};
use tokenmon_ui::settings::bind::{
    SettingsWindow, create_settings_window, refresh, select_opening_tab,
};
use tokenmon_ui::settings::{NativeSaveDialog, SettingsViewModel};
use tokenmon_ui::tray_icon::{MenuAction, MenuEntry, TrayBitmap};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{App, AppInputs};
use crate::overview::{OverviewStore, next_midnight, record_live};
use crate::panel::{Action, PanelState, PanelView, ViewEnv, build_view, commit_api_key};
use crate::panel_host::{SinglePanel, select_tab};
use crate::placement::{PlacementInput, fallback_icon, place_panel, resolve_anchor};
use crate::settings_host::{AppHost, AppSettingsBackend, HostParts};
use crate::startup::{PendingShow, StartupFailure, retry};
use crate::tray::{
    ClickAction, MenuCommand, Rect, TrayBackend, TrayController, TrayData, click_action,
    context_menu, icon_size, menu_command, was_foreground,
};
use crate::updates::UpdateService;

mod display;
mod panels;
mod system;
mod theme;
mod tray_wiring;

use panels::PanelWindow;
use tray_wiring::wire_tray;

/// How often the UI tick runs (tray state, live panel, rollover).
const TICK: Duration = Duration::from_secs(2);

/// Tray icon creation is retried this many times (one second apart) before
/// giving up: right after logon Explorer's taskbar may not exist yet.
const TRAY_ATTEMPTS: u32 = 5;

/// Longest the exit waits for blocking scans (transcripts, SQLite) to end.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// `--debug-panel`: log the panel placement inputs and outputs, the actual
/// window geometry after showing, and focus and click decisions, to the rolling
/// log in `%LOCALAPPDATA%/TokenMon/logs` (#81).
static DEBUG_PANEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn debug_panel() -> bool {
    DEBUG_PANEL.load(std::sync::atomic::Ordering::Relaxed)
}

/// A "show the panel" request from the pipe that may precede the UI.
static SHOW_REQUEST: PendingShow = PendingShow::new();

/// What the UI thread owns. `Rc` state is not `Send`, so background callbacks
/// (tray, menu, pipe, toast) never capture it: they post a closure that
/// reaches it through [`with_ui`] on the UI thread.
struct Ui {
    /// The one panel window; every tab is a body inside it (#81).
    panel: SinglePanel<PanelWindow>,
    state: RefCell<PanelState>,
    rows: RefCell<Vec<RowVm>>,
    cards: RefCell<Vec<OverviewCardVm>>,
    last_view: RefCell<Option<PanelView>>,
    anchor: Cell<Option<Rect>>,
    /// Whether the selected panel has keyboard focus, and when it last lost it
    /// (to tell "was in front" at a tray press; see [`was_foreground`]).
    focused: Cell<bool>,
    focus_lost_at: Cell<Option<Instant>>,
    /// The decision taken when the left button went down on the tray icon.
    pending_click: Cell<Option<ClickAction>>,
    store: Arc<OverviewStore>,
    app: Rc<App>,
    version: String,
    updates: Arc<UpdateService>,
    handle: tokio::runtime::Handle,
    settings_vm: Arc<SettingsViewModel>,
    settings_window: RefCell<Option<Rc<SettingsWindow>>>,
    tick: slint::Timer,
    me: Rc<RefCell<Weak<Ui>>>,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread with the UI state, if it is set up.
fn with_ui(f: impl FnOnce(&Rc<Ui>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = UI.with(|u| u.borrow().clone()) {
            f(&ui);
        }
    });
}

/// The OS clock: local offsets follow the zone's DST rules per instant.
static LOCAL_CLOCK: SystemClock = SystemClock;

fn now_and_clock() -> (DateTime<Utc>, &'static SystemClock) {
    (Utc::now(), &LOCAL_CLOCK)
}

/// Opens release pages in the default browser through Explorer.
struct ExplorerOpener;

impl UrlOpener for ExplorerOpener {
    fn open(&self, url: &str) {
        // Only https URLs from the release feed reach here; Explorer takes the
        // URL as one argument, so nothing is interpreted by a shell.
        if url.starts_with("https://") {
            let _ = std::process::Command::new("explorer").arg(url).spawn();
        }
    }
}

fn exe_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_default()
}

/// Logs and drops threshold alerts when WinRT toasts are unavailable.
struct LogNotifier;

impl Notifier for LogNotifier {
    fn deliver(&self, alert: &ThresholdAlert) {
        tracing::warn!(provider = %alert.provider, "alert without toast backend: {}", alert.body());
    }
}

impl Ui {
    /// The handler every panel window routes its actions to.
    fn handler(&self) -> PanelHandler {
        let weak = self.me.borrow().clone();
        Rc::new(move |event| {
            if let Some(ui) = weak.upgrade() {
                ui.on_event(&event);
            }
        })
    }

    /// The panel window, created and wired on first use. There is never a
    /// second one: tabs switch inside it.
    fn panel_window(&self) -> Option<Rc<PanelWindow>> {
        match self.panel.get_or_create() {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::error!(error = %e, "panel window failed");
                None
            }
        }
    }

    /// Escape closes the panel and focus changes are tracked for the tray
    /// click toggle. Losing focus does NOT close it: the panel stays open
    /// until the user closes it (maintainer decision on #81).
    fn wire_window_events(&self, window: &PanelWindow) {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
        let weak = self.me.borrow().clone();
        window.window().on_winit_window_event(move |_, event| {
            use winit::event::WindowEvent;
            use winit::keyboard::{Key, NamedKey};
            let Some(ui) = weak.upgrade() else {
                return EventResult::Propagate;
            };
            match event {
                WindowEvent::Focused(focused) => ui.note_focus(*focused),
                WindowEvent::KeyboardInput { event, .. }
                    if event.state.is_pressed()
                        && event.logical_key == Key::Named(NamedKey::Escape) =>
                {
                    ui.hide_all();
                }
                _ => {}
            }
            EventResult::Propagate
        });
    }

    /// Records a focus change of the selected panel.
    fn note_focus(&self, focused: bool) {
        self.focused.set(focused);
        if debug_panel() {
            tracing::info!(focused, "panel: focus changed");
        }
        if !focused {
            self.focus_lost_at.set(Some(Instant::now()));
        }
    }

    /// Whether the panel was the foreground window at this instant (see
    /// [`was_foreground`]).
    fn panel_was_foreground(&self) -> bool {
        was_foreground(
            self.focused.get(),
            self.focus_lost_at.get().map(|t| t.elapsed()),
        )
    }

    /// Brings the visible panel to the front with keyboard focus.
    fn raise_panel(&self) {
        if let Some(hwnd) = self.panel.existing().and_then(|w| w.hwnd()) {
            system::bring_to_front(hwnd);
        }
    }

    /// Applies the tray-click decision taken at press time.
    fn finish_tray_click(&self) {
        let action = self
            .pending_click
            .take()
            .unwrap_or_else(|| click_action(self.is_panel_visible(), self.panel_was_foreground()));
        match action {
            ClickAction::Show => self.show_selected(),
            ClickAction::BringToFront => self.raise_panel(),
            ClickAction::Hide => self.hide_all(),
        }
    }

    /// Switches to `provider`'s tab and shows the panel.
    fn show_tab(&self, provider: ProviderId) {
        select_tab(&mut self.state.borrow_mut(), provider, &|p| {
            self.app.settings.set_selected_provider(p);
        });
        self.render(true);
    }

    /// Shows the panel on the selected tab.
    fn show_selected(&self) {
        let provider = self.state.borrow().selected;
        self.show_tab(provider);
    }

    /// Builds the selected provider's view and applies it; with `reveal` the
    /// window is placed near the tray and shown. Without it, nothing happens
    /// unless the view changed.
    fn render(&self, reveal: bool) {
        let provider = self.state.borrow().selected;
        let (now, clock) = now_and_clock();
        let view = {
            let state = self.state.borrow();
            build_view(
                &ViewEnv {
                    model: &self.app.model,
                    sessions: &self.app.sessions,
                    overview: &self.store,
                    version: &self.version,
                    now,
                    clock,
                },
                &state,
                provider,
            )
        };
        let rows = build_switcher(
            &self.app.settings.snapshot().visible_usage_providers(),
            provider,
        );
        if !reveal && self.last_view.borrow().as_ref() == Some(&view) && *self.rows.borrow() == rows
        {
            return;
        }
        let Some(window) = self.panel_window() else {
            return;
        };
        if let PanelView::Overview(vm) = &view {
            self.cards.borrow_mut().clone_from(&vm.cards);
        }
        window.apply(&view, &rows);
        *self.rows.borrow_mut() = rows;
        *self.last_view.borrow_mut() = Some(view);
        if !reveal && window.is_visible() {
            // New content can change the panel's height: re-fit it against
            // the same anchor so it never grows down into the taskbar.
            self.place(&window);
        }
        if reveal {
            self.place(&window);
            window.show();
            if let Some(hwnd) = window.hwnd() {
                system::make_tool_window(hwnd);
                let accepted = system::bring_to_front(hwnd);
                if debug_panel() {
                    tracing::info!(accepted, "panel: bring to front after show");
                }
            }
            debug_assert_eq!(self.panel.creations(), 1, "one panel window only");
            self.log_actual_geometry(&window);
            self.app.model.set_menu_open(true);
        }
    }

    /// Anchors the window to the tray icon on the side away from the taskbar,
    /// inside the monitor's work area, at that monitor's DPI.
    fn place(&self, window: &PanelWindow) {
        let anchor = self.anchor.get().unwrap_or_else(default_anchor);
        let facts = display::monitor_at(center(anchor).0, center(anchor).1);
        let placement = place_panel(&PlacementInput {
            icon: anchor,
            monitor: facts.monitor,
            work: facts.work,
            dpi: facts.dpi,
            logical_size: window.logical_size(),
        });
        window.window().set_position(slint::PhysicalPosition::new(
            placement.origin.0,
            placement.origin.1,
        ));
        if debug_panel() {
            tracing::info!(
                anchor = ?anchor,
                monitor = ?facts.monitor,
                work = ?facts.work,
                dpi = facts.dpi,
                logical_size = ?window.logical_size(),
                edge = ?placement.edge,
                size = ?placement.size,
                gap = placement.gap,
                origin = ?placement.origin,
                "panel: place"
            );
        }
    }

    /// `--debug-panel`: what the window really is after showing, to compare
    /// with the placement that was computed.
    fn log_actual_geometry(&self, window: &PanelWindow) {
        if !debug_panel() {
            return;
        }
        let (size, pos) = (window.window().size(), window.window().position());
        tracing::info!(
            actual_size = ?(size.width, size.height),
            actual_position = ?(pos.x, pos.y),
            scale_factor = window.window().scale_factor(),
            hwnd = ?window.hwnd(),
            panel_windows_created = self.panel.creations(),
            "panel: shown (one panel window)"
        );
    }

    fn is_panel_visible(&self) -> bool {
        self.panel.existing().is_some_and(|w| w.is_visible())
    }

    fn hide_all(&self) {
        if let Some(w) = self.panel.existing().filter(|w| w.is_visible()) {
            w.hide();
        }
        self.focused.set(false);
        // Also when nothing is visible: a window closed another way (Alt+F4)
        // must not leave the pollers on the active cadence.
        self.app.model.sync_menu_open(false);
    }

    /// Acts on a "show" request that arrived over the pipe.
    fn show_pending(&self) {
        if SHOW_REQUEST.take() {
            self.show_selected();
        }
    }

    fn on_event(&self, event: &PanelEvent) {
        let actions = {
            let (rows, cards) = (self.rows.borrow(), self.cards.borrow());
            self.state.borrow_mut().handle(event, &rows, &cards)
        };
        for action in actions {
            self.perform(action);
        }
    }

    fn perform(&self, action: Action) {
        match action {
            Action::Redraw => self.render(false),
            Action::Show(p) => self.show_tab(p),
            Action::OpenSettings => {
                self.hide_all();
                self.open_settings();
            }
            Action::SignIn(p) => {
                // A failure is logged and kept with the session; the redraw
                // shows it in the panel.
                let _ = self.app.signin.start(p);
                self.render(false);
            }
            Action::SignOut(p) => {
                self.app.signin.sign_out(p);
                self.render(false);
            }
            Action::ToggleSection(section) => {
                let expanded = self.app.settings.snapshot().is_expanded(section);
                self.app.settings.set_expanded(!expanded, section);
                self.render(false);
            }
            Action::SubmitApiKey(key) => {
                let accepted =
                    commit_api_key(&mut self.state.borrow_mut(), &self.app.sessions, &key);
                if accepted {
                    self.app.model.sign_in_completed(ProviderId::OpenRouter);
                }
                self.render(false);
            }
        }
    }

    fn open_settings(&self) {
        let existing = self.settings_window.borrow().clone();
        let window = match existing {
            Some(w) => w,
            None => match create_settings_window(Arc::clone(&self.settings_vm)) {
                Ok(w) => {
                    *self.settings_window.borrow_mut() = Some(Rc::clone(&w));
                    w
                }
                Err(e) => {
                    tracing::error!(error = %e, "settings window failed");
                    return;
                }
            },
        };
        refresh(&window, &self.settings_vm);
        select_opening_tab(&window, &self.settings_vm);
        let _ = window.show();
    }

    fn run_command(&self, command: MenuCommand) {
        match command {
            MenuCommand::Show(p) => self.show_tab(p),
            MenuCommand::OpenSettings => self.open_settings(),
            MenuCommand::CheckForUpdates => {
                let checker = Arc::clone(&self.updates.checker);
                self.handle
                    .spawn(async move { checker.check_manually().await });
            }
            MenuCommand::Quit => self.quit(),
        }
    }

    fn quit(&self) {
        let (now, clock) = now_and_clock();
        record_live(
            &self.store,
            self.app.model.registry(),
            &self.app.settings.snapshot(),
            &self.app.sessions,
            now,
            clock,
        );
        self.app.model.quit();
        let _ = slint::quit_event_loop();
    }
}

/// The seam that builds the one panel window: wires its actions and keys to
/// the UI the first time the shell needs it.
fn panel_factory(me: &Rc<RefCell<Weak<Ui>>>) -> crate::panel_host::WindowFactory<PanelWindow> {
    let me = Rc::clone(me);
    Box::new(move || {
        let weak = me.borrow().clone();
        let ui = weak.upgrade().ok_or("the UI is gone")?;
        let closer = weak.clone();
        let on_close: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(ui) = closer.upgrade() {
                ui.hide_all();
            }
        });
        let window = PanelWindow::create(&ui.handler(), &on_close).map_err(|e| e.to_string())?;
        ui.wire_window_events(&window);
        Ok(Rc::new(window))
    })
}

/// Centre of a rectangle.
fn center(r: Rect) -> (i32, i32) {
    (r.x + r.width / 2, r.y + r.height / 2)
}

/// Where the panel opens when no tray position is known yet (a toast click or
/// `--show` before any tray event): beside the primary taskbar's corner.
fn default_anchor() -> Rect {
    let facts = display::monitor_at(0, 0);
    fallback_icon(facts.monitor, facts.work, facts.dpi)
}

/// Entry point of the Windows build. Returns the process exit code.
pub fn run(args: &[String], version: &str) -> ExitCode {
    let exe = std::env::current_exe().unwrap_or_default();
    // First: wait out a replaced predecessor and tidy update leftovers.
    handle_startup_args(args.iter().map(String::as_str), &exe);
    let launch = LaunchArgs::parse_lenient(args.iter().skip(1));
    DEBUG_PANEL.store(
        args.iter().any(|a| a == "--debug-panel"),
        std::sync::atomic::Ordering::Relaxed,
    );
    if debug_panel() {
        tracing::info!("panel: --debug-panel on (version {version})");
    }
    // A second launch asks the running instance to show its panel; that
    // instance needs foreground rights for the window to come to the front.
    system::allow_any_process_foreground();
    // Before the first AppPaths: data files are created owner-only.
    register_owner_only_permissions();
    let (pipe, client) = match current_user_pipe_name()
        .and_then(|n| PipeClient::for_current_user().map(|c| (n, c)))
    {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(error = %e, "no per-user pipe name");
            return ExitCode::FAILURE;
        }
    };
    match decide_startup(launch, &MutexInstanceLock, &client) {
        Ok(StartupDecision::RunPrimary { guard, show_panel }) => {
            let code = run_primary(&pipe, show_panel, version);
            drop(guard);
            code
        }
        Ok(StartupDecision::HandedOff) => ExitCode::SUCCESS,
        Ok(StartupDecision::HandoffFailed(e)) => {
            tracing::error!(error = %e, "could not reach the running instance");
            ExitCode::FAILURE
        }
        Err(e) => {
            tracing::error!(error = %e, "single-instance check failed");
            ExitCode::FAILURE
        }
    }
}

/// Tells the user startup failed (the app has no console), then the caller
/// returns, which releases the single-instance lock.
fn fail(failure: &StartupFailure) -> ExitCode {
    tracing::error!("{}", failure.message());
    system::error_box(crate::startup::FAILURE_TITLE, &failure.message());
    ExitCode::FAILURE
}

fn run_primary(pipe: &str, show_panel: bool, version: &str) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return fail(&StartupFailure::Assembly(e.to_string())),
    };
    let enter = runtime.enter();
    // The pipe goes up right after the lock so a second launch during the
    // (slower) rest of startup is not refused; a request that arrives before
    // the UI exists is kept in `SHOW_REQUEST`.
    let server = match PipeServer::start(pipe, |message| {
        if message == IpcMessage::Show && SHOW_REQUEST.request() {
            with_ui(|ui| ui.show_pending());
        }
    }) {
        Ok(s) => s,
        Err(e) => return fail(&StartupFailure::Pipe(e.to_string())),
    };
    let paths = match AppPaths::production() {
        Ok(p) => p,
        Err(e) => return fail(&StartupFailure::Assembly(e.to_string())),
    };
    let dir = match paths.directory() {
        Ok(d) => d,
        Err(e) => return fail(&StartupFailure::Assembly(e.to_string())),
    };
    if let Err(e) = AumidRegistration::new(Arc::new(WinRegistry), None).ensure() {
        tracing::warn!(error = %e, "AUMID registration failed; toasts may not show");
    }

    // Toast clicks arrive on a WinRT thread: marshal to the UI thread and open
    // that provider's panel.
    let toast_clicks = Arc::new(move |provider: ProviderId| {
        tracing::info!(%provider, "toast clicked");
        with_ui(move |ui| ui.show_tab(provider));
    });
    let notifier: Arc<dyn Notifier> = match WinRtToastBackend::new() {
        Ok(backend) => Arc::new(ToastNotifier::new(backend, toast_clicks)),
        Err(e) => {
            tracing::warn!(error = %e, "toasts unavailable");
            Arc::new(LogNotifier)
        }
    };

    let app = match App::build(
        AppInputs {
            paths,
            encryptor: Arc::new(DpapiEncryptor),
            web_host: Arc::new(WindowsWebAuthHost::new(dir.join("signin"))),
            notifier,
            clock: Arc::new(SystemClock),
            version: version.to_owned(),
            home: std::env::var_os("USERPROFILE").map(Into::into),
            claude_config_dir: std::env::var("CLAUDE_CONFIG_DIR").ok(),
            opencode_env: DataDirEnv::from_process(),
        },
        runtime.handle().clone(),
    ) {
        Ok(app) => Rc::new(app),
        Err(e) => return fail(&StartupFailure::Assembly(e.to_string())),
    };
    let mut tasks = app.launch();
    let updates = match UpdateService::production(
        &app.settings,
        version,
        exe_path(),
        Arc::new(ExplorerOpener),
    ) {
        Ok(u) => Arc::new(u),
        Err(e) => return fail(&StartupFailure::Assembly(e)),
    };
    tasks.push(updates.spawn_loop(runtime.handle()));

    // Turning "Check automatically" on or off applies at once.
    let checker = Arc::clone(&updates.checker);
    let _updates_observer = crate::observe::ObservationLoop::start(
        runtime.handle(),
        app.settings.subscribe(),
        Duration::from_millis(100),
        false,
        |s| s.checks_for_updates,
        move |_| {
            checker.settings_changed();
        },
    );

    // Sleep pauses every loop; wake resumes them once the network is back.
    let events = WindowsEventSource::new();
    events.start(Arc::new(SystemWakeGate::new(
        Arc::new(TokioSleeper),
        runtime.handle().clone(),
        app.wake_callbacks(),
    )));

    let settings_vm = settings_view_model(&app, &updates, version, runtime.handle());
    let store = Arc::new(OverviewStore::open(&dir));
    let selected = app.settings.snapshot().selected_provider;
    let me: Rc<RefCell<Weak<Ui>>> = Rc::default();
    let ui = Rc::new(Ui {
        panel: SinglePanel::new(panel_factory(&me)),
        state: RefCell::new(PanelState::new(selected)),
        rows: RefCell::new(Vec::new()),
        cards: RefCell::new(Vec::new()),
        last_view: RefCell::new(None),
        anchor: Cell::new(None),
        focused: Cell::new(false),
        focus_lost_at: Cell::new(None),
        pending_click: Cell::new(None),
        store,
        app: Rc::clone(&app),
        version: version.to_owned(),
        updates,
        handle: runtime.handle().clone(),
        settings_vm,
        settings_window: RefCell::new(None),
        tick: slint::Timer::default(),
        me: Rc::clone(&me),
    });
    *ui.me.borrow_mut() = Rc::downgrade(&ui);
    UI.with(|u| *u.borrow_mut() = Some(Rc::clone(&ui)));
    // Logoff, shutdown and restart must run the quit path too.
    if let Err(e) = system::create_session_window(|| {
        if let Some(ui) = UI.with(|u| u.borrow().clone()) {
            ui.quit();
        }
    }) {
        tracing::warn!(error = %e, "session-end window unavailable; logoff will not flush");
    }
    let show_panel = SHOW_REQUEST.mark_ready() || show_panel;
    if let Err(failure) = wire_tray(&ui, show_panel) {
        return fail(&failure);
    }

    let result = slint::run_event_loop_until_quit();
    ui.quit();
    // Drop the UI (its timer owns the tray icon, so NIM_DELETE runs) before
    // the process ends, then stop the workers without waiting forever for a
    // blocking scan.
    UI.with(|u| u.borrow_mut().take());
    drop(ui);
    drop(server);
    for t in &tasks {
        t.abort();
    }
    drop(enter);
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The Settings window's view model over the real services.
fn settings_view_model(
    app: &Rc<App>,
    updates: &Arc<UpdateService>,
    version: &str,
    handle: &tokio::runtime::Handle,
) -> Arc<SettingsViewModel> {
    let exe = exe_path().to_string_lossy().into_owned();
    let login = Arc::new(RunKeyLoginItem::new(Arc::new(WinRegistry), exe.clone()));
    // A moved or updated install leaves a stale Run entry; point it here.
    if login.is_stale().unwrap_or(false) {
        let _ = login.register();
    }
    let host = AppHost::new(HostParts {
        settings: Arc::clone(&app.settings),
        model: Arc::clone(&app.model),
        sessions: Arc::clone(&app.sessions),
        signin: Arc::clone(&app.signin),
        updates: Arc::clone(updates),
        login_item: login as Arc<dyn LoginItemService>,
        desktop_verb: Arc::new(DesktopVerb::new(Arc::new(WinRegistry), exe)),
        history: app.model.registry().history(),
        version: version.to_owned(),
        tz: Arc::new(|at| SystemClock.local_offset_at(at)),
        handle: handle.clone(),
        quit: Arc::new(|| with_ui(|ui| ui.quit())),
    });
    Arc::new(SettingsViewModel::new(
        Arc::new(AppSettingsBackend::new(Arc::clone(&app.settings))),
        Arc::new(host),
        Arc::new(NativeSaveDialog),
    ))
}
