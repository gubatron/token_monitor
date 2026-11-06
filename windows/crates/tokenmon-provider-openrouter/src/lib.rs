//! TokenMon provider: OpenRouter.
//!
//! API-key based usage (spec 03 sections 4 to 8): the key is stored as a
//! secret through the `CredentialStore` contract, `/key` always answers,
//! `/credits` and `/activity` only for management keys. The snapshot picks a
//! budget (account credits first, then the per-key limit with its reset
//! window) and groups per-model spend.
//!
//! The API key never appears in `Debug`, logs or error strings; see
//! [`ApiKey`].

pub mod activity;
pub mod auth;
pub mod client;
pub mod models;
pub mod poller;
pub mod snapshot;

pub use activity::{CostEstimator, ModelUsage, NoCostEstimator, canonical_slug, models_from_rows};
pub use auth::{ApiKey, OpenRouterAuthSession, SaveKeyError, looks_like_api_key};
pub use client::{Endpoint, EndpointFetcher, OpenRouterUsageClient, ReqwestFetcher};
pub use models::{ActivityRow, CreditsData, KeyData, parse_activity, parse_credits, parse_key};
pub use poller::{OpenRouterUsagePoller, RefreshOutcome};
pub use snapshot::{BudgetSource, LimitReset, OpenRouterSnapshot};
