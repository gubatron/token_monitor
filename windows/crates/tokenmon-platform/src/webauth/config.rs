//! Per-provider sign-in configuration (`ProviderSignInConfig`, spec 04 4.1).

use std::fmt;
use std::time::Duration;

use super::cookies::CapturePolicy;
use super::gate::{AuthHostFn, ReturnPageFn};

/// Default sign-in window size.
pub const WINDOW_SIZE: (u32, u32) = (920, 700);
/// Minimum content size.
pub const WINDOW_MIN_SIZE: (u32, u32) = (880, 640);
/// Default delay between the return page and the capture.
pub const DEFAULT_RETURN_DELAY: Duration = Duration::from_millis(800);
/// Grok needs a longer return delay.
pub const GROK_RETURN_DELAY: Duration = Duration::from_millis(1500);

/// Everything one provider's sign-in window needs. Provider slices construct
/// it (hosts, predicates, allowlist); this crate runs it.
#[derive(Clone)]
pub struct SignInConfig {
    /// Lowercase slug of the *session* (Grokbot uses Cursor's); keys the
    /// isolated profile folder.
    pub provider_slug: String,
    /// Window title and header title.
    pub title: String,
    /// Header text while waiting.
    pub subtitle: String,
    /// First URL to load (must be http(s)).
    pub start_url: String,
    /// Delay between detecting the return page and capturing.
    pub return_delay: Duration,
    /// `isAuthHost(host, path)`.
    pub is_auth_host: AuthHostFn,
    /// `isReturnPage(url)`.
    pub is_return_page: ReturnPageFn,
    /// Cookie capture rules.
    pub policy: CapturePolicy,
}

impl fmt::Debug for SignInConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignInConfig")
            .field("provider_slug", &self.provider_slug)
            .field("title", &self.title)
            .field("start_url", &self.start_url)
            .field("return_delay", &self.return_delay)
            .finish_non_exhaustive()
    }
}
