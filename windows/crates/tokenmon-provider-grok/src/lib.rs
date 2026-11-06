//! TokenMon provider: SuperGrok weekly pool (workstream ws-grok, spec 02
//! section 1).
//!
//! * [`grpc`]: the gRPC-web `GetGrokCreditsConfig` parser. It reads untrusted
//!   bytes, so every read is bounded to the message end, offsets use checked
//!   arithmetic and recursion is capped (upstream fix #51).
//! * [`rest`]: the REST fallback JSON parser.
//! * [`client`]: the billing call plus REST probes over an injectable
//!   [`client::HttpTransport`].
//! * [`models`]: snapshots, products and the product catalog.
//! * [`daily`] / [`calendar`]: billing-period week bounds and daily bars.
//! * [`history`]: the `grok_history.dat` store.
//! * [`export`]: CSV and JSON export.
//! * [`auth`]: sign-in constants, host and cookie policy.
//! * [`poller`]: refresh gating and displayed state.

pub mod auth;
pub mod calendar;
pub mod client;
pub mod daily;
pub mod export;
pub mod grpc;
pub mod history;
pub mod models;
pub mod poller;
pub mod rest;
