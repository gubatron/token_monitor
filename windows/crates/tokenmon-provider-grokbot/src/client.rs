//! The Grokbot fetch pipeline (spec 02 sections 2.1 and 2.2).
//!
//! [`GrokbotUsageClient`] performs the one POST. [`GrokbotFetcher`] binds it
//! to the **Cursor** cookie session: Grokbot rejections count against Cursor's
//! three-strike counter, except a non-challenge 403, which is a transient
//! "no access" error and never a strike.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde_json::json;
use tokenmon_core::{AuthSession, Clock, ProviderError, ProviderErrorContext, UsageFetcher};
use tokenmon_provider_cursor::{CURSOR_BASE_URL, CursorHttp, RequestPolicy};

use crate::models::GrokbotSnapshot;
use crate::parse::parse_usage_status;

const CTX: ProviderErrorContext = ProviderErrorContext::Grokbot;
/// Path of the Sand (Grok Bot) usage endpoint.
pub const USAGE_PATH: &str = "/api/dashboard/get-sand-usage-status";
/// `Referer` sent with the request.
pub const REFERER: &str = "https://cursor.com/bot";

/// Talks to `cursor.com` for the Grokbot allowance.
#[derive(Debug, Clone)]
pub struct GrokbotUsageClient {
    http: CursorHttp,
}

impl GrokbotUsageClient {
    /// A client over `http`.
    pub fn new(http: CursorHttp) -> Self {
        Self { http }
    }

    /// Production client for `https://cursor.com`; `user_agent` is
    /// `TokenMon/<version>`.
    pub fn production(user_agent: &str) -> Self {
        Self::new(CursorHttp::new(CURSOR_BASE_URL, user_agent))
    }

    /// `fetchUsageStatus`: POST `{}` and parse the reply.
    ///
    /// # Errors
    /// Per spec 02 section 2.2: transport gives `Network`, a challenge gives a
    /// transient `BadResponse`, a plain 403 gives the "not available" custom
    /// error, 401 and login pages give `Unauthorized`.
    pub async fn fetch_usage_status(
        &self,
        cookie: &str,
        account_email: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<GrokbotSnapshot, ProviderError> {
        let body = self
            .http
            .post_json(
                USAGE_PATH,
                cookie,
                REFERER,
                &json!({}),
                RequestPolicy::GROKBOT,
            )
            .await?;
        parse_usage_status(&body, account_email, now)
    }
}

/// [`UsageFetcher`] for Grokbot bound to the shared Cursor session.
pub struct GrokbotFetcher {
    client: GrokbotUsageClient,
    session: Arc<dyn AuthSession>,
    clock: Arc<dyn Clock>,
    last: Mutex<Option<GrokbotSnapshot>>,
}

impl GrokbotFetcher {
    /// Binds `client` to the Cursor `session`.
    pub fn new(
        client: GrokbotUsageClient,
        session: Arc<dyn AuthSession>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            client,
            session,
            clock,
            last: Mutex::new(None),
        }
    }

    /// Forgets the last snapshot (sign-out). History stores are untouched.
    pub fn clear_snapshot(&self) {
        *self.lock() = None;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<GrokbotSnapshot>> {
        self.last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl UsageFetcher for GrokbotFetcher {
    type Snapshot = GrokbotSnapshot;

    async fn fetch(&self) -> Result<GrokbotSnapshot, ProviderError> {
        let cookie = self
            .session
            .cookie_header()
            .filter(|c| !c.is_empty())
            .ok_or(ProviderError::NotSignedIn(CTX))?;
        // The generation guard: a 401 that arrives after the user signed out
        // and in again belongs to the old cookie and must not strike the new
        // session.
        let state = self.session.state();
        let generation = state.session_generation;
        let result = self
            .client
            .fetch_usage_status(&cookie, state.account_email.as_deref(), self.clock.now())
            .await;
        match result {
            Ok(snapshot) => {
                self.session.record_auth_success();
                *self.lock() = Some(snapshot.clone());
                Ok(snapshot)
            }
            Err(error) => {
                if matches!(error, ProviderError::Unauthorized(_))
                    && self.session.is_current(generation)
                {
                    self.session
                        .record_auth_failure("Cursor rejected the session");
                }
                Err(error)
            }
        }
    }

    fn snapshot(&self) -> Option<GrokbotSnapshot> {
        self.lock().clone()
    }
}
