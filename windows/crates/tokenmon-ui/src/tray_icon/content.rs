//! What the tray icon shows, derived from the app state.

use tokenmon_core::ProviderId;

/// Maximum number of mini-bars in the composite / Overview icon.
pub const MAX_BARS: usize = 4;

/// One mini-bar: a provider and its headline used percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarSegment {
    /// The provider whose accent colors the bar.
    pub provider: ProviderId,
    /// Used percent (0..=100); `None` when there is no data (draws the
    /// 2 px sliver only).
    pub used: Option<f64>,
}

/// The semantic content of the icon, before size and theme are applied.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayContent {
    /// Nothing to show (no enabled providers): an em dash.
    Empty,
    /// One provider: its logo plus a used-percent bar.
    Selected {
        /// Never [`ProviderId::Overview`].
        provider: ProviderId,
        /// Used percent, or `None` for no data (or Grok signed out).
        used: Option<f64>,
    },
    /// Up to [`MAX_BARS`] vertical bars in provider accents.
    Bars(Vec<BarSegment>),
}

/// App state the content is derived from.
#[derive(Debug, Clone)]
pub struct TrayInputs<'a> {
    /// Selected provider (may be Overview).
    pub selected: ProviderId,
    /// `showSelectedProvider`: draw only the selected provider.
    pub show_selected: bool,
    /// Headline used percent per provider that has data.
    pub used_percent: &'a [(ProviderId, f64)],
    /// User provider order (normalized internally).
    pub provider_order: &'a [ProviderId],
    /// Providers whose menu-bar graph is on.
    pub menu_bar_graphs: &'a [ProviderId],
    /// Providers that are enabled.
    pub enabled: &'a [ProviderId],
    /// Grok is signed in; when false Grok has no value.
    pub grok_signed_in: bool,
}

impl TrayContent {
    /// Chooses the content.
    ///
    /// Mode A (`show_selected` with a real provider) draws that provider.
    /// Otherwise (composite, or Overview selected) the bars are the enabled
    /// providers with a graph, in normalized order, first [`MAX_BARS`] only.
    /// Grok is always part of the composite when enabled, as on macOS.
    ///
    /// ```
    /// use tokenmon_core::ProviderId::*;
    /// use tokenmon_ui::tray_icon::{TrayContent, TrayInputs};
    /// let c = TrayContent::from_inputs(&TrayInputs {
    ///     selected: Claude, show_selected: true, used_percent: &[(Claude, 63.0)],
    ///     provider_order: &[], menu_bar_graphs: &[], enabled: &[Claude], grok_signed_in: true,
    /// });
    /// assert_eq!(c, TrayContent::Selected { provider: Claude, used: Some(63.0) });
    /// ```
    pub fn from_inputs(i: &TrayInputs<'_>) -> Self {
        let used = |p: ProviderId| {
            if p == ProviderId::Grok && !i.grok_signed_in {
                return None;
            }
            i.used_percent
                .iter()
                .find(|(q, _)| *q == p)
                .map(|(_, v)| *v)
        };
        if i.show_selected && i.selected != ProviderId::Overview {
            return Self::Selected {
                provider: i.selected,
                used: used(i.selected),
            };
        }
        let bars: Vec<BarSegment> = ProviderId::normalized_order(i.provider_order)
            .into_iter()
            .filter(|p| i.enabled.contains(p))
            .filter(|p| {
                *p == ProviderId::Grok
                    || (p.supports_menu_bar_graph() && i.menu_bar_graphs.contains(p))
            })
            .take(MAX_BARS)
            .map(|provider| BarSegment {
                provider,
                used: used(provider),
            })
            .collect();
        if bars.is_empty() {
            Self::Empty
        } else {
            Self::Bars(bars)
        }
    }

    /// Cache-key fragment; percentages are rounded as drawn.
    pub(crate) fn key(&self) -> String {
        let pct = |u: Option<f64>| u.map_or(-1, |v| v.clamp(0.0, 100.0).round() as i32);
        match self {
            Self::Empty => "empty".to_owned(),
            Self::Selected { provider, used } => format!("sel-{provider}-{}", pct(*used)),
            Self::Bars(b) => {
                let parts: Vec<String> = b
                    .iter()
                    .map(|s| format!("{}:{}", s.provider, pct(s.used)))
                    .collect();
                format!("bars-{}", parts.join(","))
            }
        }
    }
}
