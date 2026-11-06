//! Windows glue: single-instance startup, the tray icon, the panel window and
//! the platform collaborators. Everything decidable lives in the portable
//! modules; this one only connects them to the OS.
//!
//! Threads: a tokio runtime runs polling and IO; the Slint event loop runs on
//! the main thread and owns the tray and the panel. Background callbacks
//! (pipe server, toast click) reach the UI through
//! `slint::invoke_from_event_loop`.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

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
use tokenmon_platform::shell::{IpcMessage, LaunchArgs, StartupDecision, decide_startup};
use tokenmon_platform::webauth::WindowsWebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;
use tokenmon_ui::panels::{bridge, events, ui};
use tokenmon_ui::tray_icon::{MenuAction, MenuEntry, TrayBitmap, TrayIconSize};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{App, AppInputs};
use crate::panel;
use crate::tray::{TrayBackend, TrayController, TrayData, context_menu, panel_target};
use crate::updates::UpdateService;

mod theme;

/// What the UI thread owns. `Rc` state is not `Send`, so background callbacks
/// (tray, menu, pipe, toast) never capture it: they post a closure that
/// reaches it through [`with_ui`] on the UI thread.
struct Ui {
    panel: Rc<Panel>,
    app: Rc<App>,
    version: String,
    updates: Arc<UpdateService>,
    handle: tokio::runtime::Handle,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread with the UI state, if it is set up.
fn with_ui(f: impl FnOnce(&Ui) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = UI.with(|u| u.borrow().clone()) {
            f(&ui);
        }
    });
}

impl Ui {
    /// Shows the panel (anchored to the last tray position).
    fn show(&self) {
        self.panel.show(&self.app, &self.version);
    }
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
    fn update(&mut self, bitmap: &TrayBitmap, tooltip: &str) {
        match Icon::from_rgba(bitmap.rgba.clone(), bitmap.size, bitmap.size) {
            Ok(icon) => {
                let _ = self.0.set_icon(Some(icon));
            }
            Err(e) => tracing::warn!(error = %e, "tray icon bitmap rejected"),
        }
        let _ = self.0.set_tooltip(Some(tooltip));
    }
}

fn tray_menu(settings: &tokenmon_core::settings::Settings) -> Menu {
    let menu = Menu::new();
    for entry in context_menu(settings) {
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

/// Panel window and where it was last anchored.
struct Panel {
    window: ui::OverviewPanel,
    anchor: RefCell<Option<(i32, i32)>>,
}

impl Panel {
    /// Fills the panel from the live model and shows it near the tray.
    fn show(&self, app: &App, version: &str) {
        let now = chrono::Utc::now();
        let tz = *chrono::Local::now().offset();
        bridge::apply_overview(
            &self.window,
            &panel::overview(&app.model, &app.sessions, version, now, tz),
        );
        self.place();
        let _ = self.window.show();
        app.model.set_menu_open(true);
    }

    fn hide(&self, app: &App) {
        let _ = self.window.hide();
        app.model.set_menu_open(false);
    }

    fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    /// Anchors the window above (or below) the tray icon, 420 wide.
    fn place(&self) {
        let size = self.window.window().size();
        let (ax, ay) = self.anchor.borrow().unwrap_or((0, 0));
        let x = (ax - i32::try_from(size.width / 2).unwrap_or(0)).max(0);
        let y = if ay > i32::try_from(size.height).unwrap_or(0) {
            ay - i32::try_from(size.height).unwrap_or(0) - 8
        } else {
            ay + 40
        };
        self.window
            .window()
            .set_position(slint::PhysicalPosition::new(x, y.max(0)));
    }
}

/// Entry point of the Windows build. Returns the process exit code.
pub fn run(args: &[String], version: &str) -> ExitCode {
    let exe = std::env::current_exe().unwrap_or_default();
    // First: wait out a replaced predecessor and tidy update leftovers.
    handle_startup_args(args.iter().map(String::as_str), &exe);
    let launch = LaunchArgs::parse_lenient(args.iter().skip(1));
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

fn run_primary(pipe: &str, show_panel: bool, version: &str) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        return ExitCode::FAILURE;
    };
    let _enter = runtime.enter();
    let Ok(paths) = AppPaths::production() else {
        tracing::error!("no local app data directory");
        return ExitCode::FAILURE;
    };
    let Ok(dir) = paths.directory() else {
        return ExitCode::FAILURE;
    };
    if let Err(e) = AumidRegistration::new(Arc::new(WinRegistry), None).ensure() {
        tracing::warn!(error = %e, "AUMID registration failed; toasts may not show");
    }

    // Toast clicks arrive on a WinRT thread: marshal to the UI thread.
    let toast_clicks = Arc::new(move |provider: ProviderId| {
        tracing::info!(%provider, "toast clicked");
        with_ui(Ui::show);
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
        Err(e) => {
            tracing::error!(error = %e, "assembly failed");
            return ExitCode::FAILURE;
        }
    };
    let mut tasks = app.launch();
    let updates = match UpdateService::production(
        &app.settings,
        version,
        exe_path(),
        Arc::new(ExplorerOpener),
    ) {
        Ok(u) => Arc::new(u),
        Err(e) => {
            tracing::error!(error = %e, "update service unavailable");
            return ExitCode::FAILURE;
        }
    };
    tasks.push(updates.spawn_loop(runtime.handle()));

    // Sleep pauses every loop; wake resumes them once the network is back.
    let events = WindowsEventSource::new();
    events.start(Arc::new(SystemWakeGate::new(
        Arc::new(TokioSleeper),
        runtime.handle().clone(),
        app.wake_callbacks(),
    )));

    let window = match ui::OverviewPanel::new() {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "panel window failed");
            return ExitCode::FAILURE;
        }
    };
    let panel = Rc::new(Panel {
        window,
        anchor: RefCell::new(None),
    });
    let ui = Rc::new(Ui {
        panel,
        app: Rc::clone(&app),
        version: version.to_owned(),
        updates,
        handle: runtime.handle().clone(),
    });
    UI.with(|u| *u.borrow_mut() = Some(Rc::clone(&ui)));
    wire_panel(&ui);
    wire_tray(&ui, show_panel);

    // A second launch with --show asks the running instance to open the panel.
    let _server = PipeServer::start(pipe, move |message| {
        if message == IpcMessage::Show {
            with_ui(Ui::show);
        }
    });

    let result = slint::run_event_loop_until_quit();
    app.model.quit();
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Dismiss on Escape or focus loss, and route the panel's actions.
fn wire_panel(ui: &Rc<Ui>) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
    let weak = Rc::downgrade(ui);
    ui.panel
        .window
        .window()
        .on_winit_window_event(move |_, event| {
            use winit::event::WindowEvent;
            use winit::keyboard::{Key, NamedKey};
            let dismiss = match event {
                WindowEvent::Focused(false) => true,
                WindowEvent::KeyboardInput { event, .. } => {
                    event.state.is_pressed() && event.logical_key == Key::Named(NamedKey::Escape)
                }
                _ => false,
            };
            if dismiss && let Some(ui) = weak.upgrade() {
                ui.panel.hide(&ui.app);
            }
            EventResult::Propagate
        });
    let weak = Rc::downgrade(ui);
    events::connect_overview(
        &ui.panel.window,
        &(Rc::new(move |event| {
            if let Some(ui) = weak.upgrade() {
                handle_panel_event(&ui, &event);
            }
        }) as events::PanelHandler),
    );
}

fn handle_panel_event(ui: &Ui, event: &events::PanelEvent) {
    use events::PanelEvent;
    let provider = match event {
        PanelEvent::SignIn(p) | PanelEvent::SignInAgain(p) => Some(*p),
        PanelEvent::OverviewSignIn(i) => card_provider(ui, *i),
        PanelEvent::SignOut(p) => {
            ui.app.signin.sign_out(*p);
            return;
        }
        PanelEvent::OverviewSignOut(i) => {
            if let Some(p) = card_provider(ui, *i) {
                ui.app.signin.sign_out(p);
            }
            return;
        }
        _ => None,
    };
    if let Some(p) = provider
        && let Err(e) = ui.app.signin.start(p)
    {
        tracing::warn!(error = %e, provider = %p, "sign-in could not open");
    }
}

/// The provider behind Overview card `index`.
fn card_provider(ui: &Ui, index: usize) -> Option<ProviderId> {
    panel::card_inputs(&ui.app.model, &ui.app.sessions)
        .get(index)
        .map(|c| c.provider)
}

/// Creates the tray icon and routes its clicks and menu.
fn wire_tray(ui: &Rc<Ui>, show_panel: bool) {
    let settings = ui.app.settings.snapshot();
    let Ok(icon) = Icon::from_rgba(vec![0; 16 * 16 * 4], 16, 16) else {
        return;
    };
    let tray = match TrayIconBuilder::new()
        .with_icon(icon)
        .with_tooltip("TokenMon")
        .with_menu(Box::new(tray_menu(&settings)))
        .with_menu_on_left_click(false)
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "tray icon failed");
            return;
        }
    };
    let controller = Rc::new(RefCell::new(TrayController::new(TrayIconBackend(tray))));

    // Clicking the icon toggles the panel, anchored to it.
    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } = event
        {
            let anchor = (
                (rect.position.x + f64::from(rect.size.width) / 2.0) as i32,
                rect.position.y as i32,
            );
            with_ui(move |ui| {
                *ui.panel.anchor.borrow_mut() = Some(anchor);
                if ui.panel.is_visible() {
                    ui.panel.hide(&ui.app);
                } else {
                    ui.show();
                }
            });
        }
    }));

    // Right-click menu actions.
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let Some(action) = event.id.0.parse().ok().and_then(MenuAction::from_id) else {
            return;
        };
        with_ui(move |ui| match action {
            MenuAction::Quit => {
                ui.app.model.quit();
                let _ = slint::quit_event_loop();
            }
            MenuAction::CheckForUpdates => {
                let checker = Arc::clone(&ui.updates.checker);
                ui.handle
                    .spawn(async move { checker.check_manually().await });
            }
            other if panel_target(other).is_some() => ui.show(),
            _ => {}
        });
    }));

    // Re-render when usage, settings or the taskbar theme change; one short
    // timer covers all three (the theme is read from the registry).
    let timer = slint::Timer::default();
    let weak = Rc::downgrade(ui);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(2),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
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
            controller.borrow_mut().refresh(
                &TrayData {
                    settings: &settings,
                    used_percent: &used,
                    signed_in: &signed_in,
                },
                TrayIconSize::Px16,
                theme::taskbar_theme(),
            );
        },
    );
    // The timer must outlive this function.
    std::mem::forget(timer);

    if show_panel {
        ui.show();
    }
}
