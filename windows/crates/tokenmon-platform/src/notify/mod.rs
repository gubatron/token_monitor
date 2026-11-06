//! WinRT toast notifications under the TokenMon AppUserModelID.
//!
//! Owner: ws-notify (spec 01 sections 5 and 8).
//!
//! The alert text, title and tag come from `tokenmon_core::alerts`, so the
//! strings stay identical to macOS. This module adds:
//!
//! * [`toast`]: the toast XML and activation-argument format (pure, portable).
//! * [`aumid`]: first-run registration of the AppUserModelID in HKCU, through
//!   the shell [`Registry`](crate::shell::Registry) trait.
//! * [`ToastNotifier`]: the [`Notifier`] implementation, generic over a
//!   [`ToastBackend`]. The Windows backend lives in `windows.rs`
//!   (`cfg(windows)`); [`FakeToastBackend`] records toasts and simulates
//!   clicks so everything else is tested on macOS and Linux.
//!
//! Clicking a toast calls the [`ActivationHandler`] with the provider the
//! alert was about, so the app can open the panel on that provider.

pub mod aumid;
pub mod toast;
#[cfg(windows)]
pub mod windows;

use std::sync::{Arc, Mutex, PoisonError};

use tokenmon_core::ProviderId;
use tokenmon_core::alerts::{Notifier, ThresholdAlert};

pub use aumid::{AumidRegistration, AumidStatus};
pub use toast::{APP_USER_MODEL_ID, ToastContent, parse_activation_arguments};

/// Called when the user clicks a toast, with the provider it was about.
pub type ActivationHandler = Arc<dyn Fn(ProviderId) + Send + Sync>;

/// Errors from the notification layer.
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// The OS refused to show the toast.
    #[error("could not show toast: {0}")]
    Show(String),
    /// Registering the AppUserModelID failed.
    #[error(transparent)]
    Registration(#[from] crate::shell::ShellError),
}

/// Shows one composed toast. The Windows implementation talks to WinRT.
pub trait ToastBackend: Send + Sync {
    /// Shows `content`; `on_activated` must be called with the provider when
    /// the user clicks the toast (it may be called from any thread).
    fn show(
        &self,
        content: &ToastContent,
        on_activated: ActivationHandler,
    ) -> Result<(), NotifyError>;
}

/// The production [`Notifier`]: composes the toast and hands it to a backend.
pub struct ToastNotifier<B: ToastBackend> {
    backend: B,
    on_activated: ActivationHandler,
}

impl<B: ToastBackend> ToastNotifier<B> {
    /// Creates a notifier; `on_activated` runs when a toast is clicked.
    pub fn new(backend: B, on_activated: ActivationHandler) -> Self {
        Self {
            backend,
            on_activated,
        }
    }
}

impl<B: ToastBackend> Notifier for ToastNotifier<B> {
    fn deliver(&self, alert: &ThresholdAlert) {
        let content = ToastContent::from_alert(alert);
        if let Err(e) = self.backend.show(&content, self.on_activated.clone()) {
            tracing::warn!(error = %e, tag = %content.tag, "threshold toast failed");
        }
    }
}

/// A [`ToastBackend`] that records toasts and can simulate a click.
///
/// Use it as `Arc<FakeToastBackend>` so a test keeps a handle after handing
/// the backend to a [`ToastNotifier`].
#[derive(Default)]
pub struct FakeToastBackend {
    shown: Mutex<Vec<(ToastContent, ActivationHandler)>>,
    fail: Mutex<bool>,
}

impl FakeToastBackend {
    /// Creates an empty fake.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes every subsequent `show` fail.
    pub fn set_fail(&self, fail: bool) {
        *self.fail.lock().unwrap_or_else(PoisonError::into_inner) = fail;
    }

    /// Toasts shown so far, oldest first.
    #[must_use]
    pub fn shown(&self) -> Vec<ToastContent> {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(c, _)| c.clone())
            .collect()
    }

    /// Simulates the user clicking toast number `index` (its launch arguments
    /// go through the same parser the Windows backend uses).
    pub fn click(&self, index: usize) {
        let entry = self
            .shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
            .cloned();
        if let Some((content, handler)) = entry
            && let Some(provider) = parse_activation_arguments(&content.launch_arguments)
        {
            handler(provider);
        }
    }
}

impl ToastBackend for Arc<FakeToastBackend> {
    fn show(
        &self,
        content: &ToastContent,
        on_activated: ActivationHandler,
    ) -> Result<(), NotifyError> {
        if *self.fail.lock().unwrap_or_else(PoisonError::into_inner) {
            return Err(NotifyError::Show("injected failure".into()));
        }
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((content.clone(), on_activated));
        Ok(())
    }
}
