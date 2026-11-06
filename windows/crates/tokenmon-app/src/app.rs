//! The composition root: assembles settings, secrets, sessions, sign-in,
//! pollers, alerts and the lifecycle model in the order spec 01 section 6.1
//! requires, from injected platform collaborators.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use tokenmon_core::alerts::{Notifier, ThresholdNotifier};
use tokenmon_core::settings::{AppSettings, SettingsFile};
use tokenmon_core::storage::AppPaths;
use tokenmon_core::{BrowserStateCleaner, Clock};
use tokenmon_platform::power::GateCallbacks;
use tokenmon_platform::secrets::{APP_FALLBACK_POLICY, Encryptor, FallbackPolicy, Vault};
use tokenmon_platform::webauth::WebAuthHost;
use tokenmon_provider_opencode::db::DataDirEnv;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::alerts::AlertWatcher;
use crate::flush::{flush_on_quit, spawn_history_flusher};
use crate::model::AppModel;
use crate::providers::{BuildError, ProviderDeps, build_registry};
use crate::sessions::{HostBrowserCleaner, Sessions};
use crate::signin::SignInService;

/// Everything the app needs from its environment. Production fills these
/// with the Windows implementations, tests with fakes.
pub struct AppInputs {
    /// The data directory.
    pub paths: AppPaths,
    /// DPAPI on Windows, a fake in tests.
    pub encryptor: Arc<dyn Encryptor>,
    /// Plaintext-fallback policy for the vault.
    pub fallback: FallbackPolicy,
    /// The embedded sign-in browser.
    pub web_host: Arc<dyn WebAuthHost>,
    /// Delivers threshold alerts (toasts).
    pub notifier: Arc<dyn Notifier>,
    /// The time source.
    pub clock: Arc<dyn Clock>,
    /// App version.
    pub version: String,
    /// The user's home directory.
    pub home: Option<PathBuf>,
    /// Where OpenCode's database is looked up.
    pub opencode_env: DataDirEnv,
}

impl AppInputs {
    /// Default policy constant used by production wiring.
    pub const DEFAULT_FALLBACK: FallbackPolicy = APP_FALLBACK_POLICY;
}

/// The assembled application.
pub struct App {
    /// The lifecycle model.
    pub model: Arc<AppModel>,
    /// The credential sessions.
    pub sessions: Arc<Sessions>,
    /// Sign-in and sign-out.
    pub signin: Arc<SignInService>,
    /// The settings store.
    pub settings: Arc<AppSettings>,
    alerts: AlertWatcher,
    handle: Handle,
}

impl App {
    /// Assembles the app. Must run inside the tokio runtime (`handle`'s):
    /// pollers capture the current handle.
    ///
    /// Order: settings, vault and sessions (which migrate legacy secrets),
    /// stores and pollers, the model, the alert notifier.
    ///
    /// # Errors
    /// The data directory or an HTTP stack could not be prepared.
    pub fn build(inputs: AppInputs, handle: Handle) -> Result<Self, BuildError> {
        let dir = inputs.paths.directory()?;
        let settings = Arc::new(AppSettings::new(Arc::new(SettingsFile::in_app_directory(
            &inputs.paths,
        )?)));

        let vault = Arc::new(Vault::new(dir.clone(), inputs.encryptor));
        let cleaner: Arc<dyn BrowserStateCleaner> =
            Arc::new(HostBrowserCleaner::new(Arc::clone(&inputs.web_host)));
        let sessions = Arc::new(Sessions::open(
            &dir,
            &vault,
            inputs.fallback,
            &inputs.clock,
            &cleaner,
        ));

        let registry = build_registry(&ProviderDeps {
            settings: &settings,
            sessions: &sessions,
            paths: &inputs.paths,
            clock: &inputs.clock,
            version: Some(&inputs.version),
            home: inputs.home,
            opencode_env: inputs.opencode_env,
        })?;
        let model = AppModel::new(registry, Arc::clone(&settings), handle.clone());
        // Rows appended since the last debounced flush must survive a quit.
        flush_on_quit(&model, model.registry().history());

        // Sign-in completion refreshes the pollers sharing the session; the
        // model exists only after the sessions, hence the late binding.
        let late: Arc<OnceLock<Weak<AppModel>>> = Arc::new(OnceLock::new());
        let _ = late.set(Arc::downgrade(&model));
        let completed = Arc::clone(&late);
        let signin = Arc::new(SignInService::new(
            inputs.web_host,
            Arc::clone(&sessions),
            Arc::new(move |provider| {
                if let Some(model) = completed.get().and_then(Weak::upgrade) {
                    model.sign_in_completed(provider);
                }
            }),
        ));

        let alerts = AlertWatcher::new(
            model.registry().clone(),
            Arc::clone(&settings),
            ThresholdNotifier::new(settings.file(), inputs.notifier),
        );
        Ok(Self {
            model,
            sessions,
            signin,
            settings,
            alerts,
            handle,
        })
    }

    /// Real-app start (spec 01 section 6.1 step 10): starts every polling
    /// loop and the settings and alert watchers. Returns the watcher tasks.
    pub fn launch(&self) -> Vec<JoinHandle<()>> {
        self.model.start_all();
        vec![
            self.model.watch_settings(),
            self.alerts.clone().spawn(&self.handle),
            spawn_history_flusher(&self.handle, self.model.registry().history()),
        ]
    }

    /// Callbacks for the system wake gate: sleep pauses every loop, ready
    /// resumes them.
    pub fn wake_callbacks(&self) -> GateCallbacks {
        let (sleep, ready) = (Arc::clone(&self.model), Arc::clone(&self.model));
        GateCallbacks {
            on_sleep: Arc::new(move || sleep.pause_all()),
            on_ready: Arc::new(move || ready.resume_all()),
        }
    }
}
