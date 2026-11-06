//! Browser navigation state and the Back policy (spec 04 4.2 and 4.3).

/// `SignInBackAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackAction {
    /// Navigate the active view back.
    GoBack,
    /// Dismiss the top popup.
    ClosePopup,
    /// Nothing to do.
    None,
}

impl BackAction {
    /// Every case.
    pub const ALL: [Self; 3] = [Self::GoBack, Self::ClosePopup, Self::None];

    /// Raw value (`goBack`, `closePopup`, `none`).
    pub const fn raw(self) -> &'static str {
        match self {
            Self::GoBack => "goBack",
            Self::ClosePopup => "closePopup",
            Self::None => "none",
        }
    }

    /// Inverse of [`raw`](Self::raw); unknown values give `None`.
    pub fn from_raw(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.raw() == raw)
    }

    /// Display name.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::GoBack => "Back",
            Self::ClosePopup => "Close popup",
            Self::None => "None",
        }
    }

    /// `SignInPopupStack.backAction`: history first, then popups.
    pub const fn decide(can_go_back: bool, popup_depth: usize) -> Self {
        if can_go_back {
            Self::GoBack
        } else if popup_depth > 0 {
            Self::ClosePopup
        } else {
            Self::None
        }
    }
}

/// What the host reports about the active view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavSnapshot {
    /// The active view has history to go back to.
    pub can_go_back: bool,
    /// The active view can go forward.
    pub can_go_forward: bool,
    /// A navigation is in flight.
    pub is_loading: bool,
    /// Current URL of the active view.
    pub current_url: Option<String>,
}

/// Published navigation state (snapshot plus popup depth).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavState {
    /// Active-view snapshot.
    pub snapshot: NavSnapshot,
    /// Open popups.
    pub popup_depth: usize,
}

impl NavState {
    /// A popup is open.
    pub const fn has_popup(&self) -> bool {
        self.popup_depth > 0
    }

    /// Back button enabled: `canGoBack || hasPopup`.
    pub const fn back_is_enabled(&self) -> bool {
        self.snapshot.can_go_back || self.has_popup()
    }

    /// Back tooltip.
    pub const fn back_tooltip(&self) -> &'static str {
        if self.has_popup() && !self.snapshot.can_go_back {
            "Back to the sign-in page"
        } else {
            "Back"
        }
    }

    /// Address label: "Sign-in popup" while a popup is open, else
    /// `host + path` (the raw string when the URL has no host).
    pub fn address_label(&self) -> String {
        if self.has_popup() {
            return "Sign-in popup".to_owned();
        }
        let Some(raw) = self.snapshot.current_url.as_deref() else {
            return String::new();
        };
        match url::Url::parse(raw) {
            Ok(u) if u.host_str().is_some() => {
                format!("{}{}", u.host_str().unwrap_or_default(), u.path())
            }
            _ => raw.to_owned(),
        }
    }
}
