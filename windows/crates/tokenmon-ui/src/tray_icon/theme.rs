//! Taskbar light/dark theme and the trait that reads it.

/// The taskbar's theme (`SystemUsesLightTheme`, not `AppsUseLightTheme`).
///
/// It selects the "chrome" color of the icon: black on a light taskbar,
/// white on a dark one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskbarTheme {
    /// Light taskbar; draw dark glyphs.
    Light,
    /// Dark taskbar; draw light glyphs.
    Dark,
}

impl TaskbarTheme {
    /// Maps the raw registry DWORD at
    /// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme`.
    ///
    /// A missing value means a Windows build that predates the setting, whose
    /// taskbar is always dark.
    ///
    /// ```
    /// use tokenmon_ui::tray_icon::TaskbarTheme;
    /// assert_eq!(TaskbarTheme::from_system_uses_light_theme(Some(1)), TaskbarTheme::Light);
    /// assert_eq!(TaskbarTheme::from_system_uses_light_theme(Some(0)), TaskbarTheme::Dark);
    /// assert_eq!(TaskbarTheme::from_system_uses_light_theme(None), TaskbarTheme::Dark);
    /// ```
    pub const fn from_system_uses_light_theme(value: Option<u32>) -> Self {
        match value {
            Some(0) | None => Self::Dark,
            Some(_) => Self::Light,
        }
    }

    /// Stable token used in the cache key.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Chrome color as straight-alpha RGB bytes.
    pub const fn chrome_rgb(self) -> [u8; 3] {
        match self {
            Self::Light => [0, 0, 0],
            Self::Dark => [255, 255, 255],
        }
    }
}

/// Source of the current taskbar theme.
///
/// ws-app implements this over the registry (and re-queries on
/// `WM_SETTINGCHANGE` with `ImmersiveColorSet`); tests use
/// [`FixedThemeSource`].
pub trait TaskbarThemeSource {
    /// The theme the taskbar is using right now.
    fn taskbar_theme(&self) -> TaskbarTheme;
}

/// A [`TaskbarThemeSource`] that always answers the same theme.
#[derive(Debug, Clone, Copy)]
pub struct FixedThemeSource(pub TaskbarTheme);

impl TaskbarThemeSource for FixedThemeSource {
    fn taskbar_theme(&self) -> TaskbarTheme {
        self.0
    }
}
