//! HTTP client wrapper, status mapping to `UsageError`/`ProviderError`, Retry-After
//! parsing, bot-challenge detection and `Domain::matches` (spec 01 section 1.1).
//!
//! Layout, from pure to effectful:
//!
//! * `error`: the error enums and [`ProviderError::from_usage`].
//! * `retry_after`: [`parse_retry_after`] (0 to 3600 s, never panics).
//! * `detect`: [`looks_like_html`], [`looks_like_login_page`],
//!   [`is_bot_challenge`], [`is_unauthorized_message`].
//! * `domain`: [`Domain::matches`].
//! * `request`: header building and the status to [`UsageError`] mapping.
//! * `http`: [`ProviderHttp`], the `reqwest` wrapper providers call.
//!
//! Cancellation: Swift throws `CancellationError`. Here [`with_cancellation`]
//! drops the request future and returns [`ProviderError::Cancelled`], which
//! pollers classify as skipped (`ProviderError::is_cancelled`).

pub mod detect;
pub mod domain;
pub mod error;
pub mod http;
pub mod request;
pub mod retry_after;

pub use detect::{
    is_bot_challenge, is_unauthorized_message, looks_like_html, looks_like_login_page,
};
pub use domain::Domain;
pub use error::{ProviderError, ProviderErrorContext, UsageError};
pub use http::{
    DEFAULT_MAX_RESPONSE_BYTES, HttpResponse, ProviderHttp, RequestOptions, ResponseTooLarge,
    ensure_room, json_object, set_cookie_headers, split_folded_set_cookie, with_cancellation,
};
pub use request::{
    HeaderBuildError, apply_extra_headers, bot_challenge_message, build_headers, map_error,
    response_error, user_agent,
};
pub use retry_after::{MAX_RETRY_AFTER_SECS, parse_retry_after, retry_after_from_headers};
