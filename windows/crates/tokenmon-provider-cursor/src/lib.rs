//! TokenMon provider: Cursor (spec 02 sections 3 to 6).
//!
//! * [`auth`]: [`cursor_auth_config`] (shared with Grokbot) and the sign-in
//!   page classifiers.
//! * [`http`]: [`CursorHttp`], the HTTP plumbing and status mapping shared with
//!   Grokbot, plus the login-page / `not_authenticated` body checks.
//! * [`summary`]: `/api/usage-summary` and `/api/auth/me` parsers.
//! * [`events`]: usage-event page parser and event model.
//! * [`aggregate`]: cost, hourly and per-day aggregation plus the event
//!   ledger and single-entry cache.
//! * [`client`]: [`CursorUsageClient`], the fetch pipeline.
//! * [`budget`]: the daily-budget blending math.
//! * [`models`]: snapshot types.
//! * [`publish`]: poller-side carry-over rules.

pub mod aggregate;
pub mod auth;
pub mod budget;
pub mod client;
pub mod events;
pub mod http;
pub mod models;
pub mod publish;
pub mod summary;

pub use aggregate::*;
pub use auth::*;
pub use budget::*;
pub use client::*;
pub use events::*;
pub use http::*;
pub use models::*;
pub use publish::*;
pub use summary::*;
