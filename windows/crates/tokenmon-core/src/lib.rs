//! Platform-agnostic core of TokenMon for Windows.
//!
//! This crate has no OS APIs and no UI. It holds the domain model and the shared
//! contracts every other crate codes against:
//!
//! * [`provider`]: [`ProviderId`], accent colors, ordering, and the
//!   [`ProviderUsagePoller`] traits.
//! * [`auth`]: [`CredentialStore`], [`AuthConfig`], session state and events.
//! * [`clock`]: the injectable [`Clock`].
//! * [`net`]: [`UsageError`] / [`ProviderError`] (and, later, the HTTP wrapper).
//!
//! The remaining modules are documented stubs owned by other workstreams; see
//! `windows/docs/IMPLEMENTATION_PLAN.md`.

pub mod alerts;
pub mod auth;
pub mod auth_session;
pub mod clock;
pub mod net;
pub mod overview;
pub mod polling;
pub mod provider;
pub mod settings;
pub mod storage;
pub mod update;
pub mod usage;
pub mod util;

pub use auth::{
    AUTH_FAILURE_THRESHOLD, AuthConfig, AuthEvent, AuthEventListener, AuthSession,
    AuthSessionState, CredentialKey, CredentialKind, CredentialStore, CredentialStoreError,
    InMemoryCredentialStore, SessionGeneration, VAULT_UNAVAILABLE_MESSAGE,
};
pub use auth_session::{
    BrowserStateCleaner, CapturedSession, CookieAuthSession, ProviderAuthSession,
};
pub use clock::{Clock, FixedClock, ManualClock, SystemClock};
pub use net::error::{ProviderError, ProviderErrorContext, UsageError};
pub use provider::{
    BoxFuture, ParseProviderIdError, PollIntervalInputs, ProviderId, ProviderUsagePoller, Srgb,
    UsageFetcher, select_poll_interval,
};
