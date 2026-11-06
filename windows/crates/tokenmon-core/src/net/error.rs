//! Error types shared by every provider (spec 01 section 1.1.4).
//!
//! Only the variants and their `Display` text live here. The HTTP status to
//! [`UsageError`] mapping and `ProviderError::from_usage` (which turns
//! `rateLimited` into a custom error) belong to ws-core-net.

use std::time::Duration;

use thiserror::Error;

/// Why a usage request failed, independent of which provider made it.
///
/// `retry_after` is only ever present on [`UsageError::RateLimited`].
///
/// # Examples
///
/// ```
/// use tokenmon_core::UsageError;
///
/// assert_eq!(UsageError::NotSignedIn.to_string(), "Sign in to load usage.");
/// assert_eq!(UsageError::Network("timeout".into()).to_string(), "Network error: timeout");
/// ```
#[derive(Debug, Clone, PartialEq, Error)]
pub enum UsageError {
    /// No stored credentials.
    #[error("Sign in to load usage.")]
    NotSignedIn,
    /// The server rejected the credentials (HTTP 401 or equivalent).
    #[error("Session expired. Sign in again.")]
    Unauthorized,
    /// Transport failure (DNS, TLS, timeout, connection reset).
    #[error("Network error: {0}")]
    Network(String),
    /// The server answered, but not with something usable.
    #[error("Response error: {0}")]
    BadResponse(String),
    /// HTTP 429 or equivalent; `retry_after` is the server's hint in seconds.
    #[error("Rate limited. Retrying later.")]
    RateLimited {
        /// Server-provided delay in seconds, when present and sane.
        retry_after: Option<f64>,
    },
}

impl UsageError {
    /// The server's `Retry-After` hint, only for [`UsageError::RateLimited`].
    ///
    /// Returns `None` for negative, NaN or infinite hints; the caller never
    /// has to defend against them.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited {
                retry_after: Some(secs),
            } if secs.is_finite() && *secs >= 0.0 => Some(Duration::from_secs_f64(*secs)),
            _ => None,
        }
    }
}

/// Per-provider wording for errors (spec 01 section 1.1.4 table).
///
/// # Examples
///
/// ```
/// use tokenmon_core::ProviderErrorContext;
///
/// assert_eq!(ProviderErrorContext::Grok.display_name(), "Grok");
/// assert_eq!(
///     ProviderErrorContext::OpenRouter.not_signed_in_message(),
///     "Add an OpenRouter API key to load usage."
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderErrorContext {
    /// claude.ai.
    Claude,
    /// chatgpt.com.
    ChatGpt,
    /// cursor.com.
    Cursor,
    /// grok.com.
    Grok,
    /// Grok Bot billed through Cursor.
    Grokbot,
    /// OpenCode console.
    OpenCode,
    /// OpenRouter (API key, no sign-in window).
    OpenRouter,
}

impl ProviderErrorContext {
    /// Name used inside error messages.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::ChatGpt => "ChatGPT",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok",
            Self::Grokbot => "Grokbot",
            Self::OpenCode => "OpenCode console",
            Self::OpenRouter => "OpenRouter",
        }
    }

    /// Message for [`ProviderError::NotSignedIn`].
    pub const fn not_signed_in_message(self) -> &'static str {
        match self {
            Self::Claude => "Sign in to Claude to load usage.",
            Self::ChatGpt => "Sign in to ChatGPT to load usage.",
            Self::Cursor => "Sign in to Cursor to load usage.",
            Self::Grok => "Sign in to grok.com to load usage.",
            Self::Grokbot => "Sign in to Cursor to load your Grokbot allowance.",
            Self::OpenCode => "Sign in to the OpenCode console to load official Go usage.",
            Self::OpenRouter => "Add an OpenRouter API key to load usage.",
        }
    }

    /// Message for [`ProviderError::Unauthorized`].
    pub const fn unauthorized_message(self) -> &'static str {
        match self {
            Self::Claude => "Claude session expired. Sign in again.",
            Self::ChatGpt => "ChatGPT session expired. Sign in again.",
            Self::Cursor | Self::Grokbot => "Cursor session expired. Sign in again.",
            Self::Grok => "Session expired. Please sign in again.",
            Self::OpenCode => "OpenCode console session expired. Sign in again.",
            Self::OpenRouter => "OpenRouter rejected the API key. Check or replace it.",
        }
    }

    /// Message for a 403 where the provider defines one (only Grokbot does).
    pub const fn forbidden_message(self) -> Option<&'static str> {
        match self {
            Self::Grokbot => Some("Grok Bot is not available on this Cursor account."),
            _ => None,
        }
    }
}

/// A provider-flavoured error: [`UsageError`] plus the provider's own wording.
///
/// # Examples
///
/// ```
/// use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};
///
/// let e = ProviderError::Network(ProviderErrorContext::Claude, "reset".into());
/// assert_eq!(e.to_string(), "Claude network error: reset");
///
/// let custom = ProviderError::Custom {
///     message: "Claude is rate limiting requests. Retrying later.".into(),
///     usage: UsageError::RateLimited { retry_after: Some(5.0) },
/// };
/// assert_eq!(custom.usage_error().retry_after().unwrap().as_secs(), 5);
/// ```
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ProviderError {
    /// No credentials stored for this provider.
    #[error("{}", .0.not_signed_in_message())]
    NotSignedIn(ProviderErrorContext),
    /// Credentials rejected.
    #[error("{}", .0.unauthorized_message())]
    Unauthorized(ProviderErrorContext),
    /// Unusable response.
    #[error("{} response error: {}", .0.display_name(), .1)]
    BadResponse(ProviderErrorContext, String),
    /// Transport failure.
    #[error("{} network error: {}", .0.display_name(), .1)]
    Network(ProviderErrorContext, String),
    /// Pre-worded message that keeps the originating [`UsageError`] so that
    /// `retry_after` survives (used for rate limiting).
    #[error("{message}")]
    Custom {
        /// Text shown to the user.
        message: String,
        /// The original error.
        usage: UsageError,
    },
}

impl ProviderError {
    /// Wraps a generic [`UsageError`] in the wording of `context` (spec
    /// `ProviderError.init(usageError:context:)`).
    ///
    /// `RateLimited` becomes [`ProviderError::Custom`] with the message
    /// `"<name> is rate limiting requests. Retrying later."`, keeping the
    /// original [`UsageError`] so `retry_after` survives. Every other case maps
    /// one to one.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};
    ///
    /// let e = ProviderError::from_usage(
    ///     UsageError::RateLimited { retry_after: Some(30.0) },
    ///     ProviderErrorContext::Cursor,
    /// );
    /// assert_eq!(e.to_string(), "Cursor is rate limiting requests. Retrying later.");
    /// assert_eq!(e.usage_error().retry_after().unwrap().as_secs(), 30);
    /// ```
    pub fn from_usage(usage: UsageError, context: ProviderErrorContext) -> Self {
        match usage {
            UsageError::NotSignedIn => Self::NotSignedIn(context),
            UsageError::Unauthorized => Self::Unauthorized(context),
            UsageError::Network(m) => Self::Network(context, m),
            UsageError::BadResponse(m) => Self::BadResponse(context, m),
            UsageError::RateLimited { .. } => Self::Custom {
                message: format!(
                    "{} is rate limiting requests. Retrying later.",
                    context.display_name()
                ),
                usage,
            },
        }
    }

    /// The generic [`UsageError`] this error corresponds to (spec
    /// `ProviderUsageError.usageError`), for backoff and `Retry-After`.
    pub fn usage_error(&self) -> UsageError {
        match self {
            Self::NotSignedIn(_) => UsageError::NotSignedIn,
            Self::Unauthorized(_) => UsageError::Unauthorized,
            Self::BadResponse(_, m) => UsageError::BadResponse(m.clone()),
            Self::Network(_, m) => UsageError::Network(m.clone()),
            Self::Custom { usage, .. } => usage.clone(),
        }
    }
}
