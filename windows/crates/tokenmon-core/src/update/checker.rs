//! The `UpdateChecker` state machine (spec 01 sections 3.4 and 3.5).
//!
//! The checker decides *what* to do (check, offer, install or fall back to the
//! release page) and exposes the observable state the menu row binds to. All
//! side effects go through traits so the logic runs hermetically.

use std::sync::{Arc, Mutex, MutexGuard};

use super::error::{InstallError, UpdateError};
use super::feed::{
    AssetKind, AvailableRelease, ReleaseAsset, ReleaseFeedFetcher, ReleaseSource, newer_release,
};
use super::trust::is_repository_release_page;
use super::version::AppVersion;
use crate::provider::BoxFuture;

/// Reads the "check for updates automatically" setting.
pub trait UpdateSettings: Send + Sync {
    /// Whether background checks are enabled.
    fn checks_for_updates(&self) -> bool;
}

/// Opens a URL in the user's browser.
pub trait UrlOpener: Send + Sync {
    /// Opens `url`; failures are the opener's to swallow.
    fn open(&self, url: &str);
}

/// What the installer asks the app to do after a successful install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The new build is scheduled to start; the app should quit now.
    Relaunching,
    /// Installed, but launching the new build failed; ask the user to restart.
    InstalledNeedsManualRestart,
}

/// Everything the installer needs; the checker has already vetted it.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    /// The file to download.
    pub asset: ReleaseAsset,
    /// Zip or MSI.
    pub kind: AssetKind,
    /// Lowercase hex SHA-256 the download must match.
    pub expected_sha256: String,
    /// The running version.
    pub current: AppVersion,
    /// The version being installed.
    pub target: AppVersion,
}

/// The side-effecting installer (implemented in `tokenmon-platform`).
pub trait UpdateInstaller: Send + Sync {
    /// Whether the running executable can be replaced in place from where it lives.
    fn can_replace_running_app(&self) -> bool;
    /// Whether the running app was put there by the MSI. An MSI install must be
    /// updated through the MSI so Windows Installer's records stay correct; an
    /// in-place zip swap would leave them pointing at a replaced binary.
    fn installed_by_msi(&self) -> bool {
        false
    }
    /// Downloads, verifies and installs, then schedules the relaunch.
    fn install(
        &self,
        request: InstallRequest,
    ) -> BoxFuture<'_, Result<InstallOutcome, InstallError>>;
}

/// Observable checker state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateState {
    /// A newer release, when one was found.
    pub available_release: Option<AvailableRelease>,
    /// A check is in flight.
    pub is_checking: bool,
    /// An install is in flight.
    pub is_installing: bool,
    /// Transient message for the menu row.
    pub status_message: Option<String>,
    /// The last check error, `None` after a success.
    pub last_error: Option<String>,
}

impl UpdateState {
    /// Button title, in priority order: installing, checking, update, check.
    pub fn action_title(&self) -> String {
        if self.is_installing {
            "Installing update…".to_owned()
        } else if self.is_checking {
            "Checking for updates…".to_owned()
        } else if let Some(r) = &self.available_release {
            format!("Update to {}…", r.version)
        } else {
            "Check for Updates".to_owned()
        }
    }

    /// Whether the user may start another action.
    pub fn can_act(&self) -> bool {
        !self.is_checking && !self.is_installing
    }
}

/// Checks for updates and drives the install flow.
///
/// # Examples
///
/// ```
/// use tokenmon_core::update::UpdateState;
///
/// assert_eq!(UpdateState::default().action_title(), "Check for Updates");
/// ```
pub struct UpdateChecker {
    source: ReleaseSource,
    current: Option<AppVersion>,
    settings: Arc<dyn UpdateSettings>,
    fetcher: Arc<dyn ReleaseFeedFetcher>,
    opener: Arc<dyn UrlOpener>,
    state: Mutex<UpdateState>,
}

impl UpdateChecker {
    /// Builds a checker. `current` is `None` when the build has no version.
    pub fn new(
        source: ReleaseSource,
        current: Option<AppVersion>,
        settings: Arc<dyn UpdateSettings>,
        fetcher: Arc<dyn ReleaseFeedFetcher>,
        opener: Arc<dyn UrlOpener>,
    ) -> Self {
        Self {
            source,
            current,
            settings,
            fetcher,
            opener,
            state: Mutex::new(UpdateState::default()),
        }
    }

    /// A snapshot of the observable state.
    pub fn state(&self) -> UpdateState {
        self.lock().clone()
    }

    fn lock(&self) -> MutexGuard<'_, UpdateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn update(&self, f: impl FnOnce(&mut UpdateState)) {
        f(&mut self.lock());
    }

    /// The background check; skipped when automatic checks are off.
    pub async fn check_now(&self) {
        self.perform_check(false).await;
    }

    /// The user-initiated check; runs even when automatic checks are off.
    pub async fn check_manually(&self) {
        self.perform_check(true).await;
    }

    /// Reacts to the automatic-checks setting changing. Returns `true` when the
    /// caller should (re)start its polling loop, `false` when it should stop it.
    pub fn settings_changed(&self) -> bool {
        let on = self.settings.checks_for_updates();
        if !on {
            self.update(|s| {
                s.available_release = None;
                s.last_error = None;
                s.status_message = None;
            });
        }
        on
    }

    /// Installs when a release is available, otherwise checks manually.
    /// Returns `true` when the app should quit to finish a relaunch.
    pub async fn perform_primary_action(&self, installer: &dyn UpdateInstaller) -> bool {
        if self.state().available_release.is_some() {
            self.install_available_update(installer).await
        } else {
            self.check_manually().await;
            false
        }
    }

    async fn perform_check(&self, user_initiated: bool) {
        if !user_initiated && !self.settings.checks_for_updates() {
            return;
        }
        let Some(current) = self.current.clone() else {
            if user_initiated && !self.state().is_checking {
                self.update(|s| {
                    s.status_message = Some("This build has no version number to compare.".into());
                });
            }
            return;
        };
        let started = {
            let mut s = self.lock();
            if s.is_checking {
                false
            } else {
                s.is_checking = true;
                if user_initiated {
                    s.status_message = None;
                }
                true
            }
        };
        if !started {
            return;
        }
        let result = self
            .fetcher
            .fetch()
            .await
            .and_then(|body| newer_release(&body, &current, &self.source));
        let discard = !user_initiated && !self.settings.checks_for_updates();
        self.update(|s| {
            s.is_checking = false;
            if discard {
                return;
            }
            match result {
                Ok(release) => {
                    s.last_error = None;
                    s.status_message = match (&release, user_initiated) {
                        (None, true) => Some("You're up to date.".to_owned()),
                        _ => None,
                    };
                    s.available_release = release;
                }
                Err(e) => report_error(s, &e, user_initiated),
            }
        });
    }

    /// Runs the install flow for the available release (spec 3.5). Returns
    /// `true` when the app should quit to let the relaunch happen.
    pub async fn install_available_update(&self, installer: &dyn UpdateInstaller) -> bool {
        let Some(release) = self.begin_install() else {
            return false;
        };
        let quit = self.run_install(&release, installer).await;
        self.update(|s| s.is_installing = false);
        quit
    }

    /// Claims the install slot; `None` when there is nothing to do.
    fn begin_install(&self) -> Option<AvailableRelease> {
        let mut s = self.lock();
        if s.is_installing {
            return None;
        }
        let release = s.available_release.clone()?;
        s.is_installing = true;
        Some(release)
    }

    fn fall_back_to_page(&self, release: &AvailableRelease, message: &str) {
        self.update(|s| s.status_message = Some(message.to_owned()));
        // Re-checked here because `AvailableRelease` fields are public.
        let url = if is_repository_release_page(
            &release.page_url,
            &self.source.owner,
            &self.source.repository,
        ) {
            release.page_url.clone()
        } else {
            self.source.releases_page_url()
        };
        self.opener.open(&url);
    }

    async fn run_install(
        &self,
        release: &AvailableRelease,
        installer: &dyn UpdateInstaller,
    ) -> bool {
        let chosen = choose_asset(
            release,
            installer.can_replace_running_app(),
            installer.installed_by_msi(),
        );
        let Some((asset, kind)) = chosen else {
            let message = if release.archive.is_some() {
                "TokenMon cannot replace itself from this folder."
            } else {
                "No installer was published — opening the release page."
            };
            self.fall_back_to_page(release, message);
            return false;
        };
        let Some(expected) = asset.sha256.clone() else {
            self.fall_back_to_page(
                release,
                "The release has no checksum to verify — opening the release page.",
            );
            return false;
        };
        let Some(current) = self.current.clone() else {
            self.fall_back_to_page(
                release,
                "This build has no version number to check against — opening the release page.",
            );
            return false;
        };
        let request = InstallRequest {
            asset: asset.clone(),
            kind,
            expected_sha256: expected,
            current,
            target: release.version.clone(),
        };
        match installer.install(request).await {
            Ok(InstallOutcome::Relaunching) => true,
            Ok(InstallOutcome::InstalledNeedsManualRestart) => {
                self.update(|s| {
                    s.status_message = Some(format!(
                        "TokenMon {} is installed. Quit and reopen TokenMon to finish.",
                        release.version
                    ));
                });
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "update install failed");
                self.fall_back_to_page(release, &e.to_string());
                false
            }
        }
    }
}

/// Picks the asset to install (spec 3.5): an MSI install updates through the
/// MSI whenever one is published; otherwise the zip when the running exe can be
/// replaced in place, with the MSI as the fallback.
#[must_use]
pub fn choose_asset(
    release: &AvailableRelease,
    can_replace: bool,
    installed_by_msi: bool,
) -> Option<(&ReleaseAsset, AssetKind)> {
    let msi = release.installer.as_ref().map(|a| (a, AssetKind::Msi));
    let zip = release.archive.as_ref().map(|a| (a, AssetKind::Zip));
    if installed_by_msi {
        msi.or(zip.filter(|_| can_replace))
    } else if can_replace {
        zip.or(msi)
    } else {
        msi
    }
}

fn report_error(s: &mut UpdateState, e: &UpdateError, user_initiated: bool) {
    let text = e.to_string();
    if user_initiated {
        s.status_message = Some(text.clone());
    }
    s.last_error = Some(text);
}
