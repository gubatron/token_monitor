//! The immutable [`Settings`] value: every key of spec 01 section 2 with its
//! default, clamp, decode rule and derived values.
//!
//! `Settings` is a plain value. All "setters" are `with_*` methods that return a
//! new value with the clamp applied, so the rules are testable without any I/O.
//! [`AppSettings`](super::AppSettings) wraps one in a watch channel and persists it.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use serde_json::Value;

use super::file::SettingsMap;
use crate::provider::{PollIntervalInputs, ProviderId, select_poll_interval};

/// Lower and upper bound of the poll interval while the panel is open (seconds).
pub const ACTIVE_POLL_RANGE: (u32, u32) = (15, 300);
/// Lower and upper bound of the poll interval while the panel is closed (seconds).
pub const IDLE_POLL_RANGE: (u32, u32) = (60, 3600);
/// Lower and upper bound of the alert threshold (percent).
pub const THRESHOLD_RANGE: (f64, f64) = (50.0, 99.0);
/// Default active poll interval.
pub const DEFAULT_ACTIVE_POLL_SECONDS: u32 = 60;
/// Default idle poll interval.
pub const DEFAULT_IDLE_POLL_SECONDS: u32 = 300;
/// Default alert threshold.
pub const DEFAULT_THRESHOLD_PERCENT: f64 = 80.0;

/// Error returned when a string is not a known value of a settings enum.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value: {value:?}")]
pub struct ParseSettingsEnumError {
    kind: &'static str,
    value: String,
}

macro_rules! settings_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $kind:literal, { $( $(#[$vmeta:meta])* $variant:ident => $text:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];

            /// The stored (JSON) spelling.
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $text ),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseSettingsEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|v| v.as_str() == s)
                    .ok_or_else(|| ParseSettingsEnumError { kind: $kind, value: s.to_owned() })
            }
        }
    };
}

settings_enum! {
    /// A Grok product category that can be shown in the usage breakdown.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::settings::GrokProduct;
    /// assert_eq!("imagine".parse::<GrokProduct>().unwrap(), GrokProduct::Imagine);
    /// assert!("video".parse::<GrokProduct>().is_err());
    /// ```
    GrokProduct, "grok product", {
        /// Grok Build.
        Build => "build",
        /// API usage.
        Api => "api",
        /// Chat.
        Chat => "chat",
        /// Imagine.
        Imagine => "imagine",
        /// Voice.
        Voice => "voice",
        /// Everything else.
        Other => "other",
    }
}

settings_enum! {
    /// A collapsible section of the dropdown panel.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::settings::PanelSection;
    /// assert_eq!(PanelSection::ClaudeModels.as_str(), "claudeModels");
    /// ```
    PanelSection, "panel section", {
        /// The Claude models list.
        ClaudeModels => "claudeModels",
        /// The OpenCode models list.
        OpenCodeModels => "openCodeModels",
    }
}

/// Every persisted settings key (spec 01 section 2). The JSON key names are the
/// macOS `UserDefaults` names, so a settings file is readable by both ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsKey {
    /// `showCategoriesInMenuBar`.
    ShowCategoriesInMenuBar,
    /// `menuBarGraphProviders`.
    MenuBarGraphProviders,
    /// `showSelectedProviderInMenuBar`.
    ShowSelectedProviderInMenuBar,
    /// `checksForUpdates`.
    ChecksForUpdates,
    /// `activePollSeconds`.
    ActivePollSeconds,
    /// `idlePollSeconds`.
    IdlePollSeconds,
    /// `thresholdEnabled`.
    ThresholdEnabled,
    /// `thresholdPercent`.
    ThresholdPercent,
    /// `alertProviders`.
    AlertProviders,
    /// `selectedProvider`.
    SelectedProvider,
    /// `enabledProviderIDs`.
    EnabledProviderIds,
    /// `providerOrder`.
    ProviderOrder,
    /// `visibleProductIDs`.
    VisibleProductIds,
    /// `collapsedPanelSections`.
    CollapsedPanelSections,
}

impl SettingsKey {
    /// All keys.
    pub const ALL: [Self; 14] = [
        Self::ShowCategoriesInMenuBar,
        Self::MenuBarGraphProviders,
        Self::ShowSelectedProviderInMenuBar,
        Self::ChecksForUpdates,
        Self::ActivePollSeconds,
        Self::IdlePollSeconds,
        Self::ThresholdEnabled,
        Self::ThresholdPercent,
        Self::AlertProviders,
        Self::SelectedProvider,
        Self::EnabledProviderIds,
        Self::ProviderOrder,
        Self::VisibleProductIds,
        Self::CollapsedPanelSections,
    ];

    /// The JSON key.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ShowCategoriesInMenuBar => "showCategoriesInMenuBar",
            Self::MenuBarGraphProviders => "menuBarGraphProviders",
            Self::ShowSelectedProviderInMenuBar => "showSelectedProviderInMenuBar",
            Self::ChecksForUpdates => "checksForUpdates",
            Self::ActivePollSeconds => "activePollSeconds",
            Self::IdlePollSeconds => "idlePollSeconds",
            Self::ThresholdEnabled => "thresholdEnabled",
            Self::ThresholdPercent => "thresholdPercent",
            Self::AlertProviders => "alertProviders",
            Self::SelectedProvider => "selectedProvider",
            Self::EnabledProviderIds => "enabledProviderIDs",
            Self::ProviderOrder => "providerOrder",
            Self::VisibleProductIds => "visibleProductIDs",
            Self::CollapsedPanelSections => "collapsedPanelSections",
        }
    }

    /// The value of this key in `settings`, in its stored JSON form.
    pub fn encode(self, s: &Settings) -> Value {
        match self {
            Self::ShowCategoriesInMenuBar => s.show_categories_in_menu_bar.into(),
            Self::MenuBarGraphProviders => {
                strings(s.menu_bar_graph_providers.iter().map(|p| p.as_str()))
            }
            Self::ShowSelectedProviderInMenuBar => s.show_selected_provider_in_menu_bar.into(),
            Self::ChecksForUpdates => s.checks_for_updates.into(),
            Self::ActivePollSeconds => s.active_poll_seconds.into(),
            Self::IdlePollSeconds => s.idle_poll_seconds.into(),
            Self::ThresholdEnabled => s.threshold_enabled.into(),
            Self::ThresholdPercent => s.threshold_percent.into(),
            Self::AlertProviders => strings(s.alert_providers.iter().map(|p| p.as_str())),
            Self::SelectedProvider => s.selected_provider.as_str().into(),
            Self::EnabledProviderIds => strings(s.enabled_provider_ids.iter().map(|p| p.as_str())),
            // The order is the one list whose sequence is meaningful.
            Self::ProviderOrder => {
                Value::Array(s.provider_order.iter().map(|p| p.as_str().into()).collect())
            }
            Self::VisibleProductIds => strings(s.visible_product_ids.iter().map(|p| p.as_str())),
            Self::CollapsedPanelSections => {
                strings(s.collapsed_panel_sections.iter().map(|p| p.as_str()))
            }
        }
    }
}

/// Encodes a set as a JSON string array sorted by spelling (spec: "[String], sorted").
fn strings<'a>(items: impl Iterator<Item = &'a str>) -> Value {
    let mut v: Vec<&str> = items.collect();
    v.sort_unstable();
    Value::Array(v.into_iter().map(Value::from).collect())
}

/// Returned by [`Settings::with_enabled_providers`] when the proposed set is empty
/// or contains a non-usage provider; the previous value stays in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("enabled providers must be a non-empty subset of the usage providers")]
pub struct InvalidEnabledProviders;

/// A complete, valid snapshot of every user setting.
///
/// Fields are public for reading. Construct changed copies with the `with_*`
/// methods, which apply the spec's clamps; mutating a snapshot directly does not
/// persist or notify anyone.
///
/// # Examples
///
/// ```
/// use tokenmon_core::ProviderId;
/// use tokenmon_core::settings::Settings;
///
/// let s = Settings::default().with_threshold_percent(250.0);
/// assert_eq!(s.threshold_percent, 99.0);
/// assert!(s.needs_polling(ProviderId::Grok));
/// assert!(!s.needs_polling(ProviderId::Claude));
/// ```
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // one bool per independent persisted toggle
pub struct Settings {
    /// Grok category segments in the menu-bar image.
    pub show_categories_in_menu_bar: bool,
    /// Providers whose bar graph is pinned in the menu bar.
    pub menu_bar_graph_providers: BTreeSet<ProviderId>,
    /// Menu bar shows only the selected provider.
    pub show_selected_provider_in_menu_bar: bool,
    /// Automatic update checks.
    pub checks_for_updates: bool,
    /// Poll interval (s) while the panel is open; 15..=300.
    pub active_poll_seconds: u32,
    /// Poll interval (s) while the panel is closed; 60..=3600.
    pub idle_poll_seconds: u32,
    /// Usage alert on/off.
    pub threshold_enabled: bool,
    /// Alert threshold percent; 50..=99.
    pub threshold_percent: f64,
    /// Providers the alert watches.
    pub alert_providers: BTreeSet<ProviderId>,
    /// Selected dropdown tab (may be [`ProviderId::Overview`]).
    pub selected_provider: ProviderId,
    /// Providers that are switched on.
    pub enabled_provider_ids: BTreeSet<ProviderId>,
    /// Normalized tab / card order.
    pub provider_order: [ProviderId; 7],
    /// Grok product categories shown in the breakdown.
    pub visible_product_ids: BTreeSet<GrokProduct>,
    /// Closed dropdown sections.
    pub collapsed_panel_sections: BTreeSet<PanelSection>,
}

fn usage_set() -> BTreeSet<ProviderId> {
    ProviderId::USAGE_PROVIDERS.into_iter().collect()
}

fn all_products() -> BTreeSet<GrokProduct> {
    GrokProduct::ALL.iter().copied().collect()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_categories_in_menu_bar: true,
            menu_bar_graph_providers: BTreeSet::from([ProviderId::Grok]),
            show_selected_provider_in_menu_bar: false,
            checks_for_updates: true,
            active_poll_seconds: DEFAULT_ACTIVE_POLL_SECONDS,
            idle_poll_seconds: DEFAULT_IDLE_POLL_SECONDS,
            threshold_enabled: false,
            threshold_percent: DEFAULT_THRESHOLD_PERCENT,
            alert_providers: usage_set(),
            selected_provider: ProviderId::Grok,
            enabled_provider_ids: usage_set(),
            provider_order: ProviderId::USAGE_PROVIDERS,
            visible_product_ids: all_products(),
            collapsed_panel_sections: BTreeSet::new(),
        }
    }
}

/// Legacy per-provider menu-bar flags and their defaults (migration, spec 2).
pub const LEGACY_GRAPH_FLAGS: [(ProviderId, &str, bool); 5] = [
    (ProviderId::Grok, "showGrokBarInMenuBar", true),
    (ProviderId::OpenCode, "showOpenCodeBarInMenuBar", false),
    (ProviderId::Cursor, "showCursorBarInMenuBar", false),
    (ProviderId::Claude, "showClaudeBarInMenuBar", false),
    (ProviderId::Grokbot, "showGrokbotBarInMenuBar", false),
];

/// Clamps `v` to `range`, mapping NaN to the default threshold.
fn clamp_threshold(v: f64) -> f64 {
    if v.is_nan() {
        DEFAULT_THRESHOLD_PERCENT
    } else {
        v.clamp(THRESHOLD_RANGE.0, THRESHOLD_RANGE.1)
    }
}

fn clamp_seconds(v: u32, (lo, hi): (u32, u32)) -> u32 {
    v.clamp(lo, hi)
}

fn read_bool(map: &SettingsMap, key: SettingsKey, default: bool) -> bool {
    map.get(key.as_str())
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn read_seconds(map: &SettingsMap, key: SettingsKey, default: u32, range: (u32, u32)) -> u32 {
    let raw = map.get(key.as_str()).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().map(|u| i64::try_from(u).unwrap_or(i64::MAX)))
            .or_else(|| v.as_f64().map(|f| f as i64))
    });
    let clamped = raw.map_or(i64::from(default), |n| {
        n.clamp(i64::from(range.0), i64::from(range.1))
    });
    // `clamped` is within `range`, which fits u32.
    clamp_seconds(u32::try_from(clamped).unwrap_or(default), range)
}

/// Reads a string array, ignoring non-string elements; `None` when the key is
/// absent or not an array.
fn read_strings(map: &SettingsMap, key: SettingsKey) -> Option<Vec<&str>> {
    map.get(key.as_str())?
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
}

fn read_providers(map: &SettingsMap, key: SettingsKey) -> Option<Vec<ProviderId>> {
    read_strings(map, key).map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
}

impl Settings {
    /// Decodes a settings map. Total: any missing, mistyped or out-of-range
    /// value falls back to its default or clamp, so this never fails.
    ///
    /// When `menuBarGraphProviders` is absent the legacy per-provider flags are
    /// consulted (see [`LEGACY_GRAPH_FLAGS`]); removing those flags from the
    /// file is the caller's job.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::settings::{decode_map, Settings};
    ///
    /// let s = Settings::from_map(&decode_map(br#"{"thresholdPercent": -5, "activePollSeconds": 1}"#));
    /// assert_eq!(s.threshold_percent, 50.0);
    /// assert_eq!(s.active_poll_seconds, 15);
    /// ```
    pub fn from_map(map: &SettingsMap) -> Self {
        let usage = usage_set();
        let enabled: BTreeSet<ProviderId> = read_providers(map, SettingsKey::EnabledProviderIds)
            .map(|v| v.into_iter().filter(|p| usage.contains(p)).collect())
            .filter(|s: &BTreeSet<_>| !s.is_empty())
            .unwrap_or_else(usage_set);
        Self {
            show_categories_in_menu_bar: read_bool(map, SettingsKey::ShowCategoriesInMenuBar, true),
            menu_bar_graph_providers: decode_graph_providers(map),
            show_selected_provider_in_menu_bar: read_bool(
                map,
                SettingsKey::ShowSelectedProviderInMenuBar,
                false,
            ),
            checks_for_updates: read_bool(map, SettingsKey::ChecksForUpdates, true),
            active_poll_seconds: read_seconds(
                map,
                SettingsKey::ActivePollSeconds,
                DEFAULT_ACTIVE_POLL_SECONDS,
                ACTIVE_POLL_RANGE,
            ),
            idle_poll_seconds: read_seconds(
                map,
                SettingsKey::IdlePollSeconds,
                DEFAULT_IDLE_POLL_SECONDS,
                IDLE_POLL_RANGE,
            ),
            threshold_enabled: read_bool(map, SettingsKey::ThresholdEnabled, false),
            threshold_percent: map
                .get(SettingsKey::ThresholdPercent.as_str())
                .and_then(Value::as_f64)
                .map_or(DEFAULT_THRESHOLD_PERCENT, clamp_threshold),
            alert_providers: read_providers(map, SettingsKey::AlertProviders)
                .map_or_else(usage_set, |v| {
                    v.into_iter().filter(|p| usage.contains(p)).collect()
                }),
            selected_provider: map
                .get(SettingsKey::SelectedProvider.as_str())
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
                .unwrap_or(ProviderId::Grok),
            enabled_provider_ids: enabled,
            provider_order: ProviderId::normalized_order(
                &read_providers(map, SettingsKey::ProviderOrder).unwrap_or_default(),
            ),
            visible_product_ids: decode_products(map),
            collapsed_panel_sections: read_strings(map, SettingsKey::CollapsedPanelSections)
                .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
                .unwrap_or_default(),
        }
    }

    // ---- derived values -------------------------------------------------

    /// `orderedUsageProviders`.
    pub fn ordered_usage_providers(&self) -> [ProviderId; 7] {
        ProviderId::normalized_order(&self.provider_order)
    }

    /// `visibleUsageProviders`: the ordered providers that are enabled.
    pub fn visible_usage_providers(&self) -> Vec<ProviderId> {
        self.ordered_usage_providers()
            .into_iter()
            .filter(|p| self.is_provider_enabled(*p))
            .collect()
    }

    /// `isProviderEnabled(p)`.
    pub fn is_provider_enabled(&self, p: ProviderId) -> bool {
        self.enabled_provider_ids.contains(&p)
    }

    /// `alertWatches(p)`: the alert is on and `p` is one of its providers.
    pub fn alert_watches(&self, p: ProviderId) -> bool {
        self.threshold_enabled && self.alert_providers.contains(&p)
    }

    /// `showsMenuBarGraph(p)`.
    pub fn shows_menu_bar_graph(&self, p: ProviderId) -> bool {
        self.menu_bar_graph_providers.contains(&p)
    }

    /// Whether a dropdown section is expanded (`!collapsed.contains`).
    pub fn is_expanded(&self, section: PanelSection) -> bool {
        !self.collapsed_panel_sections.contains(&section)
    }

    /// `needsPolling(p)`: `p` is a real, enabled provider and at least one
    /// consumer wants its data (selected tab, Overview, a pinned bar, or an alert).
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::ProviderId::*;
    /// use tokenmon_core::settings::Settings;
    ///
    /// let s = Settings::default().with_selected_provider(Overview);
    /// assert!(s.needs_polling(Claude));
    /// assert!(!s.needs_polling(Overview));
    /// ```
    pub fn needs_polling(&self, p: ProviderId) -> bool {
        p != ProviderId::Overview
            && self.is_provider_enabled(p)
            && (self.selected_provider.polls(p)
                || self.menu_bar_graph_providers.contains(&p)
                || self.alert_watches(p))
    }

    /// `pollSeconds(menuIsOpen)`.
    pub fn poll_seconds(&self, menu_open: bool) -> u32 {
        if menu_open {
            self.active_poll_seconds
        } else {
            self.idle_poll_seconds
        }
    }

    /// `pollInterval(p, menuIsOpen)`: `None` parks the provider's loop.
    pub fn poll_interval(&self, p: ProviderId, menu_open: bool) -> Option<Duration> {
        select_poll_interval(PollIntervalInputs {
            needs_polling: self.needs_polling(p),
            menu_open,
            active_seconds: f64::from(self.active_poll_seconds),
            idle_seconds: f64::from(self.idle_poll_seconds),
        })
    }

    // ---- pure updates ---------------------------------------------------

    /// Sets the active poll interval, clamped to 15..=300.
    #[must_use]
    pub fn with_active_poll_seconds(self, seconds: u32) -> Self {
        Self {
            active_poll_seconds: clamp_seconds(seconds, ACTIVE_POLL_RANGE),
            ..self
        }
    }

    /// Sets the idle poll interval, clamped to 60..=3600.
    #[must_use]
    pub fn with_idle_poll_seconds(self, seconds: u32) -> Self {
        Self {
            idle_poll_seconds: clamp_seconds(seconds, IDLE_POLL_RANGE),
            ..self
        }
    }

    /// Sets the alert threshold, clamped to 50..=99 (NaN becomes 80).
    #[must_use]
    pub fn with_threshold_percent(self, percent: f64) -> Self {
        Self {
            threshold_percent: clamp_threshold(percent),
            ..self
        }
    }

    /// Selects a dropdown tab; unlike decoding this takes an already-valid id.
    #[must_use]
    pub fn with_selected_provider(self, p: ProviderId) -> Self {
        Self {
            selected_provider: p,
            ..self
        }
    }

    /// Replaces the provider order with its normalized form.
    #[must_use]
    pub fn with_provider_order(self, raw: &[ProviderId]) -> Self {
        Self {
            provider_order: ProviderId::normalized_order(raw),
            ..self
        }
    }

    /// `moveProvider(moving, target)`: moves `moving` so it ends at `target`'s
    /// current index. Unknown ids leave the order unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::ProviderId::*;
    /// use tokenmon_core::settings::Settings;
    ///
    /// let s = Settings::default().move_provider(Claude, Grok);
    /// assert_eq!(&s.provider_order[..2], &[Claude, Grok]);
    /// ```
    #[must_use]
    pub fn move_provider(self, moving: ProviderId, target: ProviderId) -> Self {
        let mut order = self.provider_order.to_vec();
        let (Some(from), Some(to)) = (
            order.iter().position(|p| *p == moving),
            order.iter().position(|p| *p == target),
        ) else {
            return self;
        };
        order.remove(from);
        order.insert(to, moving);
        self.with_provider_order(&order)
    }

    /// Sets the enabled providers. An empty or non-usage set is rejected. When the
    /// selected tab is switched off, selection falls back to Overview.
    pub fn with_enabled_providers(
        self,
        enabled: &BTreeSet<ProviderId>,
    ) -> Result<Self, InvalidEnabledProviders> {
        let usage = usage_set();
        if enabled.is_empty() || !enabled.is_subset(&usage) {
            return Err(InvalidEnabledProviders);
        }
        let selected = if self.selected_provider == ProviderId::Overview
            || enabled.contains(&self.selected_provider)
        {
            self.selected_provider
        } else {
            ProviderId::Overview
        };
        Ok(Self {
            enabled_provider_ids: enabled.clone(),
            selected_provider: selected,
            ..self
        })
    }

    /// Replaces the alert providers (non-usage ids are dropped; empty stays empty).
    #[must_use]
    pub fn with_alert_providers(self, providers: &BTreeSet<ProviderId>) -> Self {
        let usage = usage_set();
        Self {
            alert_providers: providers.intersection(&usage).copied().collect(),
            ..self
        }
    }

    /// `setMenuBarGraph(p, shown)`; providers without a bar graph are ignored.
    #[must_use]
    pub fn with_menu_bar_graph(self, p: ProviderId, shown: bool) -> Self {
        if !p.supports_menu_bar_graph() {
            return self;
        }
        let mut set = self.menu_bar_graph_providers.clone();
        if shown {
            set.insert(p);
        } else {
            set.remove(&p);
        }
        Self {
            menu_bar_graph_providers: set,
            ..self
        }
    }

    /// `setExpanded(bool, section)`.
    #[must_use]
    pub fn with_expanded(self, section: PanelSection, expanded: bool) -> Self {
        let mut set = self.collapsed_panel_sections.clone();
        if expanded {
            set.remove(&section);
        } else {
            set.insert(section);
        }
        Self {
            collapsed_panel_sections: set,
            ..self
        }
    }

    /// Replaces the visible Grok products (an empty set is allowed).
    #[must_use]
    pub fn with_visible_products(self, products: BTreeSet<GrokProduct>) -> Self {
        Self {
            visible_product_ids: products,
            ..self
        }
    }
}

fn decode_graph_providers(map: &SettingsMap) -> BTreeSet<ProviderId> {
    let allowed: BTreeSet<ProviderId> = ProviderId::MENU_BAR_GRAPH_PROVIDERS.into_iter().collect();
    match map.get(SettingsKey::MenuBarGraphProviders.as_str()) {
        None => migrate_graph_flags(map),
        Some(_) => read_providers(map, SettingsKey::MenuBarGraphProviders).map_or_else(
            || BTreeSet::from([ProviderId::Grok]),
            |v| v.into_iter().filter(|p| allowed.contains(p)).collect(),
        ),
    }
}

/// The set implied by the legacy per-provider flags (grok defaults to on).
pub fn migrate_graph_flags(map: &SettingsMap) -> BTreeSet<ProviderId> {
    LEGACY_GRAPH_FLAGS
        .iter()
        .filter(|(_, key, default)| map.get(*key).and_then(Value::as_bool).unwrap_or(*default))
        .map(|(p, _, _)| *p)
        .collect()
}

fn decode_products(map: &SettingsMap) -> BTreeSet<GrokProduct> {
    let Some(raw) = read_strings(map, SettingsKey::VisibleProductIds) else {
        return all_products();
    };
    if raw.is_empty() {
        return BTreeSet::new();
    }
    let kept: BTreeSet<GrokProduct> = raw
        .iter()
        .filter_map(|s| s.to_lowercase().parse().ok())
        .collect();
    if kept.is_empty() {
        all_products()
    } else {
        kept
    }
}
