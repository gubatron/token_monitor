//! TokenMon provider: Grokbot (spec 02 sections 2 and 4 to 6).
//!
//! Grokbot has no session of its own. It is billed through Cursor, so it uses
//! [`tokenmon_provider_cursor`]'s cookie session (and its three-strike
//! counter) and its HTTP plumbing.
//!
//! * [`models`]: [`GrokbotSnapshot`] and [`GrokbotEntitlement`].
//! * [`parse`]: [`parse_usage_status`], the untrusted-body parser.
//! * [`client`]: [`GrokbotUsageClient`] and the session-aware
//!   [`GrokbotFetcher`].
//! * [`poller`]: [`GrokbotUsagePoller`], the `ProviderUsagePoller` on core's
//!   `PollingLoop` over the shared Cursor session.
//! * [`publish`]: [`GrokbotPublisher`], the poller-side state: history stores,
//!   window transitions and the daily budget bars.

pub mod client;
pub mod models;
pub mod parse;
pub mod poller;
pub mod publish;

pub use client::*;
pub use models::*;
pub use parse::*;
pub use poller::*;
pub use publish::*;
