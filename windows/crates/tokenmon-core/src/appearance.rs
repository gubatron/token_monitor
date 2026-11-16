//! The app color theme (light or dark) the panels and windows render with.

/// The Windows *app* mode, the one apps follow (`AppsUseLightTheme`).
///
/// It is distinct from the taskbar theme (`SystemUsesLightTheme`), which only
/// selects the tray icon's glyph color: a user can run a dark taskbar with
/// light apps and the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppTheme {
    /// Light panels and windows.
    #[default]
    Light,
    /// Dark panels and windows.
    Dark,
}

impl AppTheme {
    /// Maps the raw registry DWORD at
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme`.
    ///
    /// `0` is dark and any other value is light. A missing value is a Windows
    /// build that predates dark mode (or an unreadable key), whose apps are
    /// light.
    ///
    /// ```
    /// use tokenmon_core::appearance::AppTheme;
    /// assert_eq!(AppTheme::from_apps_use_light_theme(Some(0)), AppTheme::Dark);
    /// assert_eq!(AppTheme::from_apps_use_light_theme(Some(1)), AppTheme::Light);
    /// assert_eq!(AppTheme::from_apps_use_light_theme(None), AppTheme::Light);
    /// ```
    pub const fn from_apps_use_light_theme(value: Option<u32>) -> Self {
        match value {
            Some(0) => Self::Dark,
            Some(_) | None => Self::Light,
        }
    }

    /// Whether this is the dark theme (the value of the UI's `Theme.dark`).
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

/// Remembers the theme last applied and reports when a new reading differs.
///
/// The shell re-reads the registry on its tick and on every theme-change
/// event, and re-styles windows only when [`ThemeTracker::update`] says the
/// theme actually changed.
#[derive(Debug, Clone, Copy, Default)]
pub struct ThemeTracker {
    applied: Option<AppTheme>,
}

impl ThemeTracker {
    /// A tracker that has applied nothing yet, so the first reading is a change.
    pub const fn new() -> Self {
        Self { applied: None }
    }

    /// Records `now`; returns it when it differs from the last one applied.
    ///
    /// ```
    /// use tokenmon_core::appearance::{AppTheme, ThemeTracker};
    /// let mut t = ThemeTracker::new();
    /// assert_eq!(t.update(AppTheme::Light), Some(AppTheme::Light));
    /// assert_eq!(t.update(AppTheme::Light), None);
    /// assert_eq!(t.update(AppTheme::Dark), Some(AppTheme::Dark));
    /// ```
    pub fn update(&mut self, now: AppTheme) -> Option<AppTheme> {
        (self.applied != Some(now)).then(|| {
            self.applied = Some(now);
            now
        })
    }

    /// The theme last applied, `None` before the first reading.
    pub const fn current(&self) -> Option<AppTheme> {
        self.applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zero is dark; every other value and a missing value is light.
    #[test]
    fn test_apps_use_light_theme_mapping() {
        assert_eq!(AppTheme::from_apps_use_light_theme(Some(0)), AppTheme::Dark);
        for v in [1, 2, u32::MAX] {
            assert_eq!(
                AppTheme::from_apps_use_light_theme(Some(v)),
                AppTheme::Light
            );
        }
        assert_eq!(AppTheme::from_apps_use_light_theme(None), AppTheme::Light);
    }

    /// The app theme must not be confused with the taskbar default: a missing
    /// value is light here (the taskbar treats it as dark).
    #[test]
    fn test_is_dark_matches_variant() {
        assert!(AppTheme::Dark.is_dark());
        assert!(!AppTheme::Light.is_dark());
        assert!(!AppTheme::default().is_dark());
    }

    /// Only a changed reading is reported, and the first one always is.
    #[test]
    fn test_tracker_reports_only_changes() {
        let mut t = ThemeTracker::new();
        assert_eq!(t.current(), None);
        assert_eq!(t.update(AppTheme::Dark), Some(AppTheme::Dark));
        assert_eq!(t.update(AppTheme::Dark), None);
        assert_eq!(t.update(AppTheme::Light), Some(AppTheme::Light));
        assert_eq!(t.current(), Some(AppTheme::Light));
    }
}
