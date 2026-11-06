//! HTTP client wrapper, status mapping to `UsageError`/`ProviderError`, Retry-After
//! parsing, bot-challenge detection and `Domain::matches`.
//!
//! Owner: ws-core-net. Seeded by ws-foundation with [`error`] (the error enums
//! only: variants and `Display`); the status-to-error mapping logic and
//! everything else in this module belongs to ws-core-net.

pub mod error;
