//! TokenMon provider: ChatGPT (Codex usage), spec 03 sections 2 and 5 to 8.
//!
//! Flow: the captured `__Secure-next-auth.session-token` cookie is exchanged at
//! `/api/auth/session` for a short-lived bearer token (cached in memory, see
//! [`AccessTokenCache`]), which authorizes `/backend-api/wham/usage` (parsed by
//! [`parse_usage`] into a [`ChatGptSnapshot`]). [`ChatGptPoller`] wraps the
//! fetch with the session-guard rules shared by every provider.
//!
//! Parity: the Swift quirks are copied on purpose (see `windows/docs/PARITY.md`
//! Quirks and Divergences), so both apps show identical numbers.

pub mod auth;
pub mod client;
pub mod jwt;
pub mod poller;
pub mod time;
pub mod token_cache;
pub mod usage;

pub use auth::{
    CHATGPT_HOSTS, ChatGptSignIn, SESSION_TOKEN_PREFIX, chatgpt_auth_config, is_auth_host,
    is_preferred_session_cookie, is_return_page, looks_like_auth_cookie,
};
pub use client::{
    BASE_URL, ChatGptTransport, ChatGptUsageClient, Fetch, HttpTransport, RawReply,
    parse_session_token,
};
pub use poller::{ChatGptPoller, FetchFn, RefreshOutcome, RollingCookieSession};
pub use token_cache::{AccessTokenCache, valid_until};
pub use usage::{ChatGptSnapshot, UsageResponse, UsageWindow, parse_usage};
