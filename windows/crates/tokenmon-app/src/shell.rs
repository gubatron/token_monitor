//! Windows glue: single-instance startup, the tray icon, the panel windows and
//! the platform collaborators. Everything decidable lives in the portable
//! modules ([`crate::panel`], [`crate::tray`], [`crate::overview`],
//! [`crate::settings_host`]); this one only connects them to the OS.
//!
//! Threads: a tokio runtime runs polling and IO; the Slint event loop runs on
//! the main thread and owns the tray, the panel windows and Settings.
//! Background callbacks (pipe server, toast click, Settings quit) reach the UI
//! through `slint::invoke_from_event_loop`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, FixedOffset, Utc};
use slint::ComponentHandle;
use tokenmon_core::alerts::{Notifier, ThresholdAlert};
use tokenmon_core::polling::TokioSleeper;
use tokenmon_core::storage::AppPaths;
use tokenmon_core::update::UrlOpener;
use tokenmon_core::{ProviderId, SystemClock};
use tokenmon_platform::install::handle_startup_args;
use tokenmon_platform::notify::windows::WinRtToastBackend;
use tokenmon_platform::notify::{AumidRegistration, ToastNotifier};
use tokenmon_platform::power::{SystemEventSource, SystemWakeGate, windows::WindowsEventSource};
use tokenmon_platform::secrets::register_owner_only_permissions;
use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, DpapiEncryptor};
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
use tokenmon_ui::settings::bind::{SettingsWindow, create_settings_window, refresh};
use tokenmon_ui::settings::{NativeSaveDialog, SettingsViewModel};
use tokenmon_ui::tray_icon::{MenuAction, MenuEntry, TrayBitmap};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{App, AppInputs};
use crate::overview::{OverviewStore, next_midnight, record_live};
use crate::panel::{Action, PanelState, PanelView, ViewEnv, build_view, commit_api_key};
use crate::settings_host::{AppHost, AppSettingsBackend, HostParts};
use crate::startup::{PendingShow, StartupFailure, retry};
use crate::tray::{
    MenuCommand, PANEL_GAP, TrayBackend, TrayController, TrayData, context_menu, icon_size,
    menu_command, place_panel,
};
use crate::updates::UpdateService;

mod display;
mod panels;
mod system;
mod theme;

use panels::PanelWindow;

/// A click on the tray icon this soon after the panel dismissed itself on
/// focus loss is the same click: it must not reopen the panel.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

/// How often the UI tick runs (tray state, live panel, rollover).
const TICK: Duration = Duration::from_secs(2);

/// Tray icon creation is retried this many times (one second apart) before
/// giving up: right after logon Explorer's taskbar may not exist yet.
const TRAY_ATTEMPTS: u32 = 5;

/// Longest the exit waits for blocking scans (transcripts, SQLite) to end.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(2);

/// A "show the panel" request from the pipe that may precede the UI.
static SHOW_REQUEST: PendingShow = PendingShow::new();

/// What the UI thread owns. `Rc` state is not `Send`, so background callbacks
/// (tray, menu, pipe, toast) never capture it: they post a closure that
/// reaches it through [`with_ui`] on the UI thread.
struct Ui {
    windows: RefCell<BTreeMap<ProviderId, Rc<PanelWindow>>>,
    state: RefCell<PanelState>,
    rows: RefCell<Vec<RowVm>>,
    cards: RefCell<Vec<OverviewCardVm>>,
    last_view: RefCell<Option<PanelView>>,
    anchor: Cell<Option<(i32, i32)>>,
    dismissed_at: Cell<Option<Instant>>,
    store: Arc<OverviewStore>,
    app: Rc<App>,
    version: String,
    updates: Arc<UpdateService>,
    handle: tokio::runtime::Handle,
    settings_vm: Arc<SettingsViewModel>,
    settings_window: RefCell<Option<Rc<SettingsWindow>>>,
    tick: slint::Timer,
    me: RefCell<Weak<Ui>>,
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

fn now_and_offset() -> (DateTime<Utc>, FixedOffset) {
    (Utc::now(), *chrono::Local::now().offset())
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

/// Drives the real tray icon.
struct TrayIconBackend(TrayIcon);

impl TrayBackend for TrayIconBackend {
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str) -> bool {
        let icon = match Icon::from_rgba(bitmap.rgba.clone(), bitmap.size, bitmap.size) {
            Ok(icon) => icon,
            Err(e) => {
                tracing::warn!(error = %e, "tray icon bitmap rejected");
                return false;
            }
        };
        // Both are attempted; a failure of either is retried on the next tick.
        let icon_set = self.0.set_icon(Some(icon));
        let tip_set = self.0.set_tooltip(Some(tooltip));
        for e in [icon_set.as_ref().err(), tip_set.as_ref().err()]
            .into_iter()
            .flatten()
        {
            tracing::warn!(error = %e, "tray icon update failed; will retry");
        }
        icon_set.is_ok() && tip_set.is_ok()
    }
}

fn tray_menu(entries: &[MenuEntry]) -> Menu {
    let menu = Menu::new();
    for entry in entries {
        match entry {
            MenuEntry::Item { label, action } => {
                let _ = menu.append(&MenuItem::with_id(
                    action.id().to_string(),
                    label,
                    true,
                    None,
                ));
            }
            MenuEntry::Separator => {
                let _ = menu.append(&PredefinedMenuItem::separator());
            }
        }
    }
    menu
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

    /// The window for `provider`, created and wired on first use.
    fn window_for(&self, provider: ProviderId) -> Option<Rc<PanelWindow>> {
        if let Some(w) = self.windows.borrow().get(&provider) {
            return Some(Rc::clone(w));
        }
        let window = match PanelWindow::create(provider, &self.handler()) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                tracing::error!(error = %e, %provider, "panel window failed");
                return None;
            }
        };
        self.wire_dismiss(provider, &window);
        self.windows
            .borrow_mut()
            .insert(provider, Rc::clone(&window));
        Some(window)
    }

    /// Dismiss on Escape or when the selected panel loses focus.
    fn wire_dismiss(&self, provider: ProviderId, window: &Rc<PanelWindow>) {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
        let weak = self.me.borrow().clone();
        window.window().on_winit_window_event(move |_, event| {
            use winit::event::WindowEvent;
            use winit::keyboard::{Key, NamedKey};
            let dismiss = match event {
                WindowEvent::Focused(false) => true,
                WindowEvent::KeyboardInput { event, .. } => {
                    event.state.is_pressed() && event.logical_key == Key::Named(NamedKey::Escape)
                }
                _ => false,
            };
            if dismiss
                    && let Some(ui) = weak.upgrade()
                    // A tab switch hides the old window after selecting the
                    // new one; only the selected panel dismisses.
                    && ui.state.borrow().selected == provider
            {
                ui.hide_all();
            }
            EventResult::Propagate
        });
    }

    /// Switches to `provider`'s tab and shows the panel.
    fn show_tab(&self, provider: ProviderId) {
        self.state.borrow_mut().selected = provider;
        self.app.settings.set_selected_provider(provider);
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
        let (now, tz) = now_and_offset();
        let view = {
            let state = self.state.borrow();
            build_view(
                &ViewEnv {
                    model: &self.app.model,
                    sessions: &self.app.sessions,
                    overview: &self.store,
                    version: &self.version,
                    now,
                    tz,
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
        let Some(window) = self.window_for(provider) else {
            return;
        };
        if let PanelView::Overview(vm) = &view {
            self.cards.borrow_mut().clone_from(&vm.cards);
        }
        window.apply(&view, &rows);
        *self.rows.borrow_mut() = rows;
        *self.last_view.borrow_mut() = Some(view);
        if reveal {
            for (p, other) in self.windows.borrow().iter() {
                if *p != provider && other.is_visible() {
                    other.hide();
                }
            }
            self.place(&window);
            window.show();
            self.app.model.set_menu_open(true);
        }
    }

    /// Anchors the window above (or below) the tray icon, inside the work area.
    fn place(&self, window: &PanelWindow) {
        let size = window.window().size();
        let anchor = self.anchor.get().unwrap_or_else(default_anchor);
        let (work, _) = display::monitor_at(anchor.0, anchor.1);
        let (x, y) = place_panel(
            anchor,
            (
                i32::try_from(size.width).unwrap_or(0),
                i32::try_from(size.height).unwrap_or(0),
            ),
            work,
        );
        window
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }

    fn is_panel_visible(&self) -> bool {
        self.windows.borrow().values().any(|w| w.is_visible())
    }

    fn hide_all(&self) {
        if self.is_panel_visible() {
            for w in self.windows.borrow().values() {
                w.hide();
            }
            self.dismissed_at.set(Some(Instant::now()));
        }
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

    fn recently_dismissed(&self) -> bool {
        self.dismissed_at
            .get()
            .is_some_and(|t| t.elapsed() < REOPEN_GUARD)
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
        let (now, tz) = now_and_offset();
        record_live(
            &self.store,
            self.app.model.registry(),
            &self.app.settings.snapshot(),
            &self.app.sessions,
            now,
            tz,
        );
        self.app.model.quit();
        let _ = slint::quit_event_loop();
    }
}

/// Where the panel opens when no tray position is known yet (a toast click or
/// `--show` before any tray event): the primary work area's bottom right.
fn default_anchor() -> (i32, i32) {
    let (work, _) = display::monitor_at(0, 0);
    (work.x + work.width - 220, work.y + work.height + PANEL_GAP)
}

/// Entry point of the Windows build. Returns the process exit code.
pub fn run(args: &[String], version: &str) -> ExitCode {
    let exe = std::env::current_exe().unwrap_or_default();
    // First: wait out a replaced predecessor and tidy update leftovers.
    handle_startup_args(args.iter().map(String::as_str), &exe);
    let launch = LaunchArgs::parse_lenient(args.iter().skip(1));
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
            fallback: APP_FALLBACK_POLICY,
            web_host: Arc::new(WindowsWebAuthHost::new(dir.join("signin"))),
            notifier,
            clock: Arc::new(SystemClock),
            version: version.to_owned(),
            home: std::env::var_os("USERPROFILE").map(Into::into),
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
    let ui = Rc::new(Ui {
        windows: RefCell::new(BTreeMap::new()),
        state: RefCell::new(PanelState::new(selected)),
        rows: RefCell::new(Vec::new()),
        cards: RefCell::new(Vec::new()),
        last_view: RefCell::new(None),
        anchor: Cell::new(None),
        dismissed_at: Cell::new(None),
        store,
        app: Rc::clone(&app),
        version: version.to_owned(),
        updates,
        handle: runtime.handle().clone(),
        settings_vm,
        settings_window: RefCell::new(None),
        tick: slint::Timer::default(),
        me: RefCell::new(Weak::new()),
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
        tz: Arc::new(|| *chrono::Local::now().offset()),
        handle: handle.clone(),
        quit: Arc::new(|| with_ui(|ui| ui.quit())),
    });
    Arc::new(SettingsViewModel::new(
        Arc::new(AppSettingsBackend::new(Arc::clone(&app.settings))),
        Arc::new(host),
        Arc::new(NativeSaveDialog),
    ))
}

/// What the periodic UI tick remembers between runs.
struct TickState {
    controller: TrayController<TrayIconBackend>,
    menu: Vec<MenuEntry>,
    next_midnight: DateTime<Utc>,
}

/// Creates the tray icon and routes its clicks and menu.
///
/// # Errors
/// [`StartupFailure::Tray`] when the icon cannot be created after retries.
fn wire_tray(ui: &Rc<Ui>, show_panel: bool) -> Result<(), StartupFailure> {
    let settings = ui.app.settings.snapshot();
    let entries = context_menu(&settings);
    let build = || -> Result<TrayIcon, String> {
        let icon = Icon::from_rgba(vec![0; 16 * 16 * 4], 16, 16).map_err(|e| e.to_string())?;
        TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("TokenMon")
            .with_menu(Box::new(tray_menu(&entries)))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())
    };
    let tray = retry(
        TRAY_ATTEMPTS,
        |failures| {
            tracing::warn!(failures, "tray icon not created yet; retrying");
            std::thread::sleep(Duration::from_secs(1));
        },
        build,
    )
    .map_err(StartupFailure::Tray)?;

    // Any tray event tells where the icon is; a left click toggles the panel.
    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        let (rect, click) = match &event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } => (*rect, true),
            TrayIconEvent::Click { rect, .. }
            | TrayIconEvent::Enter { rect, .. }
            | TrayIconEvent::Move { rect, .. }
            | TrayIconEvent::Leave { rect, .. } => (*rect, false),
            _ => return,
        };
        let anchor = (
            (rect.position.x + f64::from(rect.size.width) / 2.0) as i32,
            rect.position.y as i32,
        );
        with_ui(move |ui| {
            ui.anchor.set(Some(anchor));
            if !click {
                return;
            }
            if ui.is_panel_visible() {
                ui.hide_all();
            } else if !ui.recently_dismissed() {
                ui.show_selected();
            }
        });
    }));

    // Right-click menu actions.
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let Some(action) = event.id.0.parse().ok().and_then(MenuAction::from_id) else {
            return;
        };
        with_ui(move |ui| ui.run_command(menu_command(action)));
    }));

    let mut tick = TickState {
        controller: TrayController::new(TrayIconBackend(tray)),
        menu: entries,
        next_midnight: next_midnight(Utc::now(), *chrono::Local::now().offset()),
    };
    let weak = Rc::downgrade(ui);
    ui.tick.start(slint::TimerMode::Repeated, TICK, move || {
        if let Some(ui) = weak.upgrade() {
            on_tick(&ui, &mut tick);
        }
    });

    if show_panel {
        ui.show_selected();
    }
    Ok(())
}

/// The 2 s tick: re-renders the tray (usage, settings, taskbar theme, DPI and
/// icon-size changes all show up as a different render key; the controller
/// touches the OS only when it changed), swaps the context menu when the
/// provider set changed, keeps an open panel live, rolls the Overview over at
/// local midnight and records the Overview history.
fn on_tick(ui: &Rc<Ui>, tick: &mut TickState) {
    let (now, tz) = now_and_offset();
    let settings = ui.app.settings.snapshot();
    let used: Vec<_> = ui
        .app
        .model
        .registry()
        .entries()
        .iter()
        .filter_map(|e| e.poller.headline_used_percent().map(|v| (e.provider, v)))
        .collect();
    let sessions = Arc::clone(&ui.app.sessions);
    let signed_in = move |p: ProviderId| sessions.signed_in(p);
    let anchor = ui.anchor.get().unwrap_or((0, 0));
    let (_, dpi) = display::monitor_at(anchor.0, anchor.1);
    tick.controller.refresh(
        &TrayData {
            settings: &settings,
            used_percent: &used,
            signed_in: &signed_in,
        },
        icon_size(display::small_icon_metric(dpi), dpi),
        theme::taskbar_theme(),
    );

    let entries = context_menu(&settings);
    if entries != tick.menu {
        tick.controller
            .backend_mut()
            .0
            .set_menu(Some(Box::new(tray_menu(&entries))));
        tick.menu = entries;
    }

    if now >= tick.next_midnight {
        tick.next_midnight = next_midnight(now, tz);
        ui.state.borrow_mut().reset_offsets();
    }
    // Cheap and written only when the series changed, so a poll just before
    // midnight reaches yesterday's row within one tick.
    record_live(
        &ui.store,
        ui.app.model.registry(),
        &settings,
        &ui.app.sessions,
        now,
        tz,
    );
    // A panel hidden by some other route (Alt+F4) must not keep the pollers
    // on the active cadence.
    ui.app.model.sync_menu_open(ui.is_panel_visible());
    if ui.is_panel_visible() {
        ui.render(false);
    }
    if let Some(window) = ui.settings_window.borrow().as_ref()
        && window.window().is_visible()
    {
        refresh(window, &ui.settings_vm);
    }
}
