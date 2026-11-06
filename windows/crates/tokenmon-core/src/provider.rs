//! Provider identity, registry order, accent colors and the poller contracts.
//!
//! Source of truth: spec 04 section 1 (registry, colors) and spec 01 section
//! 1.2.4 (`ProviderUsagePoller`). Behavior that depends on persisted settings
//! (`needsPolling`, `providerOrder`) is owned by ws-settings; only the pure
//! helpers that the registry itself defines live here.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::net::error::ProviderError;

/// A boxed, sendable future; keeps [`ProviderUsagePoller`] object-safe so the
/// app can hold a heterogeneous registry of pollers.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Returned when a string does not name a [`ProviderId`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown provider id: {0:?}")]
pub struct ParseProviderIdError(pub String);

/// An sRGB color with float components in `0.0..=1.0`.
///
/// The floats are the exact values from the macOS app so both platforms render
/// the same brand colors.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Srgb {
    /// Red component.
    pub red: f64,
    /// Green component.
    pub green: f64,
    /// Blue component.
    pub blue: f64,
    /// Opacity.
    pub alpha: f64,
}

impl Srgb {
    /// Opacity applied to an accent when used for bars and charts in the panel.
    pub const PANEL_ACCENT_ALPHA: f64 = 0.85;

    /// An opaque color.
    pub const fn rgb(red: f64, green: f64, blue: f64) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    /// The same color with a different opacity.
    #[must_use]
    pub const fn with_alpha(self, alpha: f64) -> Self {
        Self { alpha, ..self }
    }

    /// 8-bit channel values `(r, g, b)`, rounded to nearest (for Slint/bitmaps).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::ProviderId;
    /// assert_eq!(ProviderId::Grok.accent().to_rgb8(), (28, 97, 209));
    /// ```
    pub fn to_rgb8(self) -> (u8, u8, u8) {
        let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (q(self.red), q(self.green), q(self.blue))
    }
}

/// Identifier of a monitored provider, plus the synthetic `Overview` mode.
///
/// The `snake`/lowercase string form is the persisted id: it appears in
/// settings keys, sign-in window ids and JSON.
///
/// # Examples
///
/// ```
/// use tokenmon_core::ProviderId;
///
/// let p: ProviderId = "chatgpt".parse().unwrap();
/// assert_eq!(p.to_string(), "chatgpt");
/// assert_eq!(p.display_name(), "ChatGPT");
/// assert_eq!(ProviderId::Grokbot.sign_in_provider(), ProviderId::Cursor);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    /// The aggregate "All" mode; never a polled provider.
    Overview,
    /// grok.com.
    Grok,
    /// cursor.com.
    Cursor,
    /// OpenCode console and local database.
    OpenCode,
    /// Claude (claude.ai and Claude Code).
    Claude,
    /// ChatGPT.
    ChatGpt,
    /// OpenRouter.
    OpenRouter,
    /// Grok Bot, billed through Cursor.
    Grokbot,
}

impl ProviderId {
    /// `MonitorProvider.allCases`: overview first, then the 7 providers.
    pub const ALL_CASES: [Self; 8] = [
        Self::Overview,
        Self::Grok,
        Self::Cursor,
        Self::OpenCode,
        Self::Claude,
        Self::ChatGpt,
        Self::OpenRouter,
        Self::Grokbot,
    ];

    /// `usageProviders`: the 7 real providers in default dropdown, registry
    /// and menu-bar composite order.
    pub const USAGE_PROVIDERS: [Self; 7] = [
        Self::Grok,
        Self::Cursor,
        Self::OpenCode,
        Self::Claude,
        Self::ChatGpt,
        Self::OpenRouter,
        Self::Grokbot,
    ];

    /// `menuBarGraphProviders`: the order Settings lists the menu-bar graph
    /// toggles. This is NOT the drawing order (drawing follows the user's
    /// normalized provider order).
    pub const MENU_BAR_GRAPH_PROVIDERS: [Self; 5] = [
        Self::Grok,
        Self::Grokbot,
        Self::Claude,
        Self::Cursor,
        Self::OpenCode,
    ];

    /// Persisted id (also the serde and `Display` form).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Grok => "grok",
            Self::Cursor => "cursor",
            Self::OpenCode => "opencode",
            Self::Claude => "claude",
            Self::ChatGpt => "chatgpt",
            Self::OpenRouter => "openrouter",
            Self::Grokbot => "grokbot",
        }
    }

    /// Human-readable name for titles and settings.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Grok => "Grok",
            Self::Cursor => "Cursor",
            Self::OpenCode => "OpenCode",
            Self::Claude => "Claude",
            Self::ChatGpt => "ChatGPT",
            Self::OpenRouter => "OpenRouter",
            Self::Grokbot => "Grokbot",
        }
    }

    /// Label on the provider switcher (`Overview` shows as "All").
    pub const fn switcher_label(self) -> &'static str {
        match self {
            Self::Overview => "All",
            other => other.display_name(),
        }
    }

    /// Stable key of the provider's monochrome logo asset, or `None` for
    /// Overview (which has no provider logo; the UI uses the TokenMon mark,
    /// see [`ProviderId::TOKENMON_MARK_ASSET_KEY`]).
    ///
    /// Keys map to files under `crates/tokenmon-ui/ui/assets/logos/<key>.svg`
    /// (ws-ui-kit owns the files). Logos are treated as template masks tinted
    /// by the caller.
    pub const fn logo_asset_key(self) -> Option<&'static str> {
        match self {
            Self::Overview => None,
            Self::Grok => Some("grok"),
            Self::Cursor => Some("cursor"),
            Self::OpenCode => Some("opencode"),
            Self::Claude => Some("anthropic"),
            Self::ChatGpt => Some("openai"),
            Self::OpenRouter => Some("openrouter"),
            Self::Grokbot => Some("grokbot"),
        }
    }

    /// Asset key of the app mascot mark (Overview header and menu-bar label).
    pub const TOKENMON_MARK_ASSET_KEY: &'static str = "tokenmon-mark";

    /// Canonical brand color, used at full alpha in the tray icon.
    pub const fn accent(self) -> Srgb {
        match self {
            Self::Overview => Srgb::rgb(0.50, 0.50, 0.50),
            Self::Grok => Srgb::rgb(0.11, 0.38, 0.82),
            Self::OpenCode => Srgb::rgb(1.00, 0.55, 0.00),
            Self::Cursor => Srgb::rgb(0.18, 0.53, 0.38),
            Self::Claude => Srgb::rgb(0.85, 0.47, 0.34),
            Self::ChatGpt => Srgb::rgb(0.16, 0.52, 0.46),
            Self::OpenRouter => Srgb::rgb(0.45, 0.36, 0.90),
            Self::Grokbot => Srgb::rgb(0.38, 0.42, 0.50),
        }
    }

    /// Accent at the panel opacity (0.85) used for bars and charts.
    pub const fn panel_accent(self) -> Srgb {
        self.accent().with_alpha(Srgb::PANEL_ACCENT_ALPHA)
    }

    /// Whether the menu bar can draw a usage graph for this provider.
    ///
    /// ChatGPT, OpenRouter and Overview cannot.
    pub const fn supports_menu_bar_graph(self) -> bool {
        matches!(
            self,
            Self::Grok | Self::Grokbot | Self::Claude | Self::Cursor | Self::OpenCode
        )
    }

    /// The provider whose sign-in session this provider uses
    /// (`Grokbot` shares Cursor's; everything else is its own).
    pub const fn sign_in_provider(self) -> Self {
        match self {
            Self::Grokbot => Self::Cursor,
            other => other,
        }
    }

    /// Whether this provider has an embedded-browser sign-in window.
    /// OpenRouter connects with an API key in Settings instead; Overview has
    /// no session at all.
    pub const fn has_sign_in_window(self) -> bool {
        !matches!(self, Self::Overview | Self::OpenRouter)
    }

    /// `polls(provider)` on a selected mode `self`: true when `self` is that
    /// provider or the Overview.
    pub fn polls(self, provider: Self) -> bool {
        self == provider || self == Self::Overview
    }

    /// `normalizedOrder(raw)`: drops Overview, duplicates and unknowns, keeps
    /// the rest in order, then appends missing providers in
    /// [`Self::USAGE_PROVIDERS`] order. Always yields exactly 7 entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::ProviderId::*;
    /// let order = tokenmon_core::ProviderId::normalized_order(&[Claude, Overview, Claude, Grok]);
    /// assert_eq!(order, [Claude, Grok, Cursor, OpenCode, ChatGpt, OpenRouter, Grokbot]);
    /// ```
    pub fn normalized_order(raw: &[Self]) -> [Self; 7] {
        let mut seen: Vec<Self> = Vec::with_capacity(7);
        for p in raw {
            if Self::USAGE_PROVIDERS.contains(p) && !seen.contains(p) {
                seen.push(*p);
            }
        }
        seen.extend(
            Self::USAGE_PROVIDERS
                .iter()
                .filter(|p| !seen.contains(p))
                .copied()
                .collect::<Vec<_>>(),
        );
        seen.try_into()
            .expect("normalized order has exactly 7 entries")
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = ParseProviderIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL_CASES
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| ParseProviderIdError(s.to_owned()))
    }
}

/// Inputs to the poll-interval decision.
///
/// ws-settings computes `needs_polling` and the two intervals from
/// `AppSettings`; the poller only consumes the result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PollIntervalInputs {
    /// `AppSettings.needsPolling(provider)`.
    pub needs_polling: bool,
    /// Whether the panel is open.
    pub menu_open: bool,
    /// Seconds between polls while the panel is open.
    pub active_seconds: f64,
    /// Seconds between polls while the panel is closed.
    pub idle_seconds: f64,
}

/// `pollInterval(p, menuIsOpen)`: `None` parks the polling loop.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use tokenmon_core::{select_poll_interval, PollIntervalInputs};
///
/// let i = PollIntervalInputs { needs_polling: true, menu_open: true, active_seconds: 60.0, idle_seconds: 300.0 };
/// assert_eq!(select_poll_interval(i), Some(Duration::from_secs(60)));
/// assert_eq!(select_poll_interval(PollIntervalInputs { needs_polling: false, ..i }), None);
/// ```
pub fn select_poll_interval(inputs: PollIntervalInputs) -> Option<Duration> {
    if !inputs.needs_polling {
        return None;
    }
    let secs = if inputs.menu_open {
        inputs.active_seconds
    } else {
        inputs.idle_seconds
    };
    (secs.is_finite() && secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Object-safe face of a provider poller (spec 01 section 1.2.4).
///
/// The app holds `Arc<dyn ProviderUsagePoller>` per provider for menu-open
/// refresh, headline percentages, alerts and wake handling. Implementations
/// use interior mutability and are shared across tasks. Snapshot access is on
/// [`UsageFetcher`], which carries the snapshot type.
pub trait ProviderUsagePoller: Send + Sync {
    /// Which provider this poller serves.
    fn provider(&self) -> ProviderId;

    /// Tells the poller whether the panel is open. On change, it must wake its
    /// polling loop so the interval switches immediately.
    fn set_menu_open(&self, open: bool);

    /// When the last refresh finished (success or failure).
    fn last_refreshed_at(&self) -> Option<DateTime<Utc>>;

    /// The last refresh error, cleared by a successful refresh.
    fn last_error(&self) -> Option<ProviderError>;

    /// Used percentage (0..=100) of the headline quota shown in the menu bar
    /// and used for alerts.
    fn headline_used_percent(&self) -> Option<f64>;

    /// When the headline quota resets.
    fn headline_resets_at(&self) -> Option<DateTime<Utc>>;

    /// Account label for alert text, if known.
    fn alert_account(&self) -> Option<String>;

    /// Starts the polling loop (refreshes at once when the interval is set).
    fn start(&self);

    /// Stops the polling loop.
    fn stop(&self);

    /// Fetches now when polling is enabled and restarts the wait.
    fn refresh_now(&self) -> BoxFuture<'_, ()>;

    /// Drops the displayed snapshot but keeps history (sign-out, reset).
    fn clear_snapshot(&self);
}

/// The typed fetch half of a poller: one network round trip producing a
/// provider-specific snapshot.
///
/// Implemented by each provider's client; the poller wraps it with state,
/// the polling loop and error recording.
pub trait UsageFetcher: Send + Sync {
    /// Provider-specific immutable snapshot (for example Grok pools or Claude
    /// windows).
    type Snapshot: Clone + Send + Sync + 'static;

    /// Fetches a fresh snapshot.
    fn fetch(&self) -> impl Future<Output = Result<Self::Snapshot, ProviderError>> + Send;

    /// The most recent successful snapshot, if any.
    fn snapshot(&self) -> Option<Self::Snapshot>;
}
