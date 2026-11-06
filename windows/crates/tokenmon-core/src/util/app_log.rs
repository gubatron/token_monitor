//! Logging identity and the `User-Agent` string.

/// Single source of truth for the logging subsystem.
#[derive(Debug, Clone, Copy)]
pub struct AppLog;

impl AppLog {
    /// Subsystem name shared with the macOS app.
    pub const SUBSYSTEM: &'static str = "com.modelmonitor.app";
}

/// Logger categories used by core code (providers add their own targets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogCategory {
    /// Cache layers.
    Cache,
    /// File-backed stores.
    FileStore,
    /// Secret storage (the vault on Windows; "Keychain" on macOS).
    Keychain,
    /// Threshold alerts.
    Alerts,
    /// Update checker.
    Update,
}

impl LogCategory {
    /// Category name as logged.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "Cache",
            Self::FileStore => "FileStore",
            Self::Keychain => "Keychain",
            Self::Alerts => "Alerts",
            Self::Update => "Update",
        }
    }

    /// `tracing` target string, `<subsystem>::<category>`.
    #[must_use]
    pub fn target(self) -> String {
        format!("{}::{}", AppLog::SUBSYSTEM, self.as_str())
    }
}

/// `TokenMon/<version>` sent as the API `User-Agent`; `unknown` without a version.
///
/// # Examples
///
/// ```
/// use tokenmon_core::util::user_agent;
/// assert_eq!(user_agent(Some("1.2.1")), "TokenMon/1.2.1");
/// assert_eq!(user_agent(None), "TokenMon/unknown");
/// ```
#[must_use]
pub fn user_agent(version: Option<&str>) -> String {
    format!("TokenMon/{}", version.unwrap_or("unknown"))
}
