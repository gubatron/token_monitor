//! Toast content, XML and activation arguments (pure, portable).

use tokenmon_core::ProviderId;
use tokenmon_core::alerts::ThresholdAlert;

/// The fixed AppUserModelID every TokenMon toast is shown under. The
/// installer's Start-menu shortcut must carry the same value.
pub const APP_USER_MODEL_ID: &str = "TokenMon";

/// `ToastNotification.Tag` is limited to 64 characters.
pub const MAX_TAG_LEN: usize = 64;

const ARGUMENT_KEY: &str = "provider=";

/// Everything needed to show one threshold toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastContent {
    /// Toast title (`"TokenMon Alert"`).
    pub title: String,
    /// Toast body, identical to the macOS notification body.
    pub body: String,
    /// Toast tag: the alert id, so a repeat replaces rather than stacks.
    pub tag: String,
    /// Launch arguments delivered back on click (`provider=<id>`).
    pub launch_arguments: String,
}

impl ToastContent {
    /// Composes the toast for `alert`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::ProviderId;
    /// use tokenmon_core::alerts::ThresholdAlert;
    /// use tokenmon_platform::notify::ToastContent;
    ///
    /// let a = ThresholdAlert { provider: ProviderId::Claude, used_percent: 85.0, threshold: 80.0 };
    /// let t = ToastContent::from_alert(&a);
    /// assert_eq!(t.title, "TokenMon Alert");
    /// assert_eq!(t.launch_arguments, "provider=claude");
    /// ```
    #[must_use]
    pub fn from_alert(alert: &ThresholdAlert) -> Self {
        Self {
            title: alert.title().to_owned(),
            body: alert.body(),
            tag: alert.id().chars().take(MAX_TAG_LEN).collect(),
            launch_arguments: format!("{ARGUMENT_KEY}{}", alert.provider.as_str()),
        }
    }

    /// The `ToastGeneric` XML document for this toast. All text is escaped.
    #[must_use]
    pub fn to_xml(&self) -> String {
        format!(
            "<toast launch=\"{}\"><visual><binding template=\"ToastGeneric\">\
             <text>{}</text><text>{}</text></binding></visual></toast>",
            xml_escape(&self.launch_arguments),
            xml_escape(&self.title),
            xml_escape(&self.body),
        )
    }
}

/// Escapes the five XML special characters (valid in text and attributes).
#[must_use]
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Parses toast launch arguments back into the provider to open.
///
/// Returns `None` for anything that is not `provider=<known id>`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::ProviderId;
/// use tokenmon_platform::notify::parse_activation_arguments;
///
/// assert_eq!(parse_activation_arguments("provider=grok"), Some(ProviderId::Grok));
/// assert_eq!(parse_activation_arguments("garbage"), None);
/// ```
#[must_use]
pub fn parse_activation_arguments(args: &str) -> Option<ProviderId> {
    args.strip_prefix(ARGUMENT_KEY)?.parse().ok()
}
