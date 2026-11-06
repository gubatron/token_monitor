//! WinRT `ToastNotificationManager` backend (Windows only).

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::core::{HSTRING, IInspectable, Interface};

use super::toast::{APP_USER_MODEL_ID, ToastContent, parse_activation_arguments};
use super::{ActivationHandler, NotifyError, ToastBackend};

/// Recent toasts kept alive so their `Activated` handlers keep firing.
const KEEP_ALIVE: usize = 16;

fn show_err(e: &windows::core::Error) -> NotifyError {
    NotifyError::Show(e.to_string())
}

/// Shows toasts through WinRT under [`APP_USER_MODEL_ID`].
pub struct WinRtToastBackend {
    live: Mutex<VecDeque<ToastNotification>>,
}

impl WinRtToastBackend {
    /// Sets the process AppUserModelID and creates the backend.
    pub fn new() -> Result<Self, NotifyError> {
        // SAFETY: the argument is a valid, NUL-terminated wide string owned
        // by the HSTRING for the duration of the call.
        unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_USER_MODEL_ID)) }
            .map_err(|e| show_err(&e))?;
        Ok(Self {
            live: Mutex::new(VecDeque::new()),
        })
    }
}

impl ToastBackend for WinRtToastBackend {
    fn show(
        &self,
        content: &ToastContent,
        on_activated: ActivationHandler,
    ) -> Result<(), NotifyError> {
        let doc = XmlDocument::new().map_err(|e| show_err(&e))?;
        doc.LoadXml(&HSTRING::from(content.to_xml()))
            .map_err(|e| show_err(&e))?;
        let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| show_err(&e))?;
        toast
            .SetTag(&HSTRING::from(content.tag.as_str()))
            .map_err(|e| show_err(&e))?;
        let handler =
            TypedEventHandler::<ToastNotification, IInspectable>::new(move |_sender, args| {
                let provider = args
                    .as_ref()
                    .and_then(|a| a.cast::<ToastActivatedEventArgs>().ok())
                    .and_then(|a| a.Arguments().ok())
                    .and_then(|s| parse_activation_arguments(&s.to_string()));
                if let Some(p) = provider {
                    on_activated(p);
                }
                Ok(())
            });
        toast.Activated(&handler).map_err(|e| show_err(&e))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))
            .and_then(|n| n.Show(&toast))
            .map_err(|e| show_err(&e))?;
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if live.len() >= KEEP_ALIVE {
            live.pop_front();
        }
        live.push_back(toast);
        Ok(())
    }
}
