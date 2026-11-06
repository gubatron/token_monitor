//! The Cursor fetch pipeline (spec 02 sections 3.2 and 3.4).
//!
//! [`CursorUsageClient::fetch_snapshot`] runs `usage-summary` and `auth/me`
//! concurrently, then adds event-derived figures through the cache/ledger in
//! [`crate::aggregate`]. [`CursorFetcher`] wraps it as a
//! [`tokenmon_core::UsageFetcher`] bound to an auth session.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::json;
use tokenmon_core::usage::Calendar;
use tokenmon_core::{AuthSession, Clock, ProviderError, ProviderErrorContext, UsageFetcher};

use crate::aggregate::{
    CursorCacheEntry, CursorDayHourlyUsage, CursorEventAggregates, CursorEventCache,
    CursorEventLedger, EVENTS_MAX_PAGES, EVENTS_PAGE_SIZE, aggregate_events, events_window_start,
};
use crate::events::{CursorUsageEvent, events_page_from_object};
use crate::http::{CURSOR_BASE_URL, CursorHttp, reject_unauthorized_body};
use crate::models::CursorSnapshot;
use crate::summary::{parse_email, parse_summary_object};

const CTX: ProviderErrorContext = ProviderErrorContext::Cursor;
const EVENTS_PATH: &str = "/api/dashboard/get-filtered-usage-events";

/// Result of one [`CursorUsageClient::fetch_snapshot`].
#[derive(Debug, Clone, PartialEq)]
pub struct CursorFetchResult {
    /// The snapshot, with `cost_stats` and `daily_tokens` when events loaded.
    pub snapshot: CursorSnapshot,
    /// Today's hourly activity; empty when events are unavailable.
    pub hourly: CursorDayHourlyUsage,
    /// Tokens per day for budget estimation; empty when unavailable.
    pub estimated_weight_by_day: BTreeMap<NaiveDate, f64>,
}

/// Talks to `cursor.com` for one signed-in account at a time.
pub struct CursorUsageClient {
    http: CursorHttp,
    clock: Arc<dyn Clock>,
    cache: CursorEventCache,
}

impl std::fmt::Debug for CursorUsageClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorUsageClient").finish_non_exhaustive()
    }
}

impl CursorUsageClient {
    /// A client over `http` using `clock` for the local calendar.
    pub fn new(http: CursorHttp, clock: Arc<dyn Clock>) -> Self {
        Self {
            http,
            clock,
            cache: CursorEventCache::default(),
        }
    }

    /// Production client for `https://cursor.com`; `app_version` feeds the
    /// `User-Agent`.
    ///
    /// # Errors
    /// `Network` when the TLS backend cannot be initialised.
    pub fn production(
        app_version: Option<&str>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ProviderError> {
        let http = CursorHttp::production(CTX, app_version, Arc::clone(&clock))?;
        Ok(Self::new(http, clock))
    }

    /// The event cache (cleared on sign-out and account reset).
    pub fn cache(&self) -> &CursorEventCache {
        &self.cache
    }

    /// `fetchSnapshot(now)`.
    ///
    /// # Errors
    /// A failing summary request fails the call (HTTP mapping per
    /// the [`CursorHttp`] status mapping). `auth/me` and events failures never do.
    pub async fn fetch_snapshot(
        &self,
        cookie: &str,
        now: DateTime<Utc>,
    ) -> Result<CursorFetchResult, ProviderError> {
        let (summary, me) = futures::future::join(
            self.http.get("/api/usage-summary", cookie, CURSOR_BASE_URL),
            self.http.get("/api/auth/me", cookie, CURSOR_BASE_URL),
        )
        .await;
        let root = reject_unauthorized_body(&summary?, CTX)?;
        let mut snapshot = parse_summary_object(&root, now);
        snapshot.account_email = me
            .and_then(|body| reject_unauthorized_body(&body, CTX).map(|_| body))
            .ok()
            .and_then(|body| parse_email(&body));

        let window_start = events_window_start(&*self.clock, snapshot.billing_cycle_start, now);
        let aggregates = self
            .event_aggregates(cookie, &snapshot, window_start, now)
            .await
            .filter(|a| a.window_start == window_start);
        let today = Calendar::local(&*self.clock).start_of_day(now);
        Ok(match aggregates {
            None => CursorFetchResult {
                snapshot,
                hourly: CursorDayHourlyUsage::empty(today),
                estimated_weight_by_day: BTreeMap::new(),
            },
            Some(a) => {
                snapshot.cost_stats = Some(a.cost_stats);
                snapshot.daily_tokens = a.tokens_by_day;
                let hourly = if a.hourly.day_start == today {
                    a.hourly
                } else {
                    CursorDayHourlyUsage::empty(today)
                };
                CursorFetchResult {
                    snapshot,
                    hourly,
                    estimated_weight_by_day: a.estimated_weight_by_day,
                }
            }
        })
    }

    /// `eventAggregates`: cached, incremental or full; on a paging failure the
    /// cached aggregates (possibly stale or `None`) are returned.
    async fn event_aggregates(
        &self,
        cookie: &str,
        snapshot: &CursorSnapshot,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Option<CursorEventAggregates> {
        let entry = self.cache.get(cookie).unwrap_or_default();
        if let Some(fresh) = entry
            .aggregates
            .as_ref()
            .filter(|a| a.is_fresh(&*self.clock, window_start, now))
        {
            return Some(fresh.clone());
        }
        let ledger = match self.refresh_ledger(cookie, &entry, window_start, now).await {
            Ok(ledger) => ledger,
            Err(error) => {
                tracing::warn!(%error, "Cursor events refresh failed; using cached aggregates");
                return entry.aggregates;
            }
        };
        let aggregates =
            aggregate_events(&*self.clock, &ledger.events, snapshot, window_start, now);
        self.cache.store(
            cookie,
            CursorCacheEntry {
                aggregates: Some(aggregates.clone()),
                ledger: Some(ledger),
            },
        );
        Some(aggregates)
    }

    async fn refresh_ledger(
        &self,
        cookie: &str,
        entry: &CursorCacheEntry,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<CursorEventLedger, ProviderError> {
        match entry
            .ledger
            .as_ref()
            .filter(|l| l.can_extend(window_start, now))
        {
            Some(cached) => {
                let from = cached.incremental_from();
                let fresh = self.fetch_all_events(cookie, from, now).await?;
                Ok(cached.extended(from, fresh, now))
            }
            None => Ok(CursorEventLedger {
                window_start,
                events: self.fetch_all_events(cookie, window_start, now).await?,
                fetched_through: now,
                full_fetch_at: now,
            }),
        }
    }

    /// `fetchAllEvents(from, to)`: pages of 500, at most 40 pages, stopping on
    /// a short page or once the reported total has been read.
    async fn fetch_all_events(
        &self,
        cookie: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<CursorUsageEvent>, ProviderError> {
        let (start_ms, end_ms) = (from.timestamp_millis(), to.timestamp_millis());
        let mut events = Vec::new();
        let mut paged = 0_usize;
        for page in 1..=EVENTS_MAX_PAGES {
            let body = json!({
                "startDate": start_ms.to_string(),
                "endDate": end_ms.to_string(),
                "page": page,
                "pageSize": EVENTS_PAGE_SIZE,
            });
            let bytes = self
                .http
                .post_json(EVENTS_PATH, cookie, CURSOR_BASE_URL, &body)
                .await?;
            let parsed = events_page_from_object(&reject_unauthorized_body(&bytes, CTX)?);
            paged += parsed.page_count;
            events.extend(parsed.events);
            let total_reached =
                parsed.total > 0 && i64::try_from(paged).is_ok_and(|p| p >= parsed.total);
            if parsed.page_count < EVENTS_PAGE_SIZE || total_reached {
                break;
            }
            if page == EVENTS_MAX_PAGES {
                tracing::warn!("Cursor events hit the {EVENTS_MAX_PAGES}-page cap");
            }
        }
        Ok(events)
    }
}

/// [`UsageFetcher`] for Cursor bound to a cookie session.
///
/// It keeps the last successful snapshot, and reports auth outcomes to the
/// session (three strikes invalidate it, a success resets the counter).
pub struct CursorFetcher {
    client: CursorUsageClient,
    session: Arc<dyn AuthSession>,
    clock: Arc<dyn Clock>,
    last: Mutex<Option<CursorFetchResult>>,
}

impl CursorFetcher {
    /// Binds `client` to `session`.
    pub fn new(
        client: CursorUsageClient,
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

    /// The underlying client (for cache clearing on sign-out).
    pub fn client(&self) -> &CursorUsageClient {
        &self.client
    }

    /// The full last result, including hourly and weight data.
    pub fn last_result(&self) -> Option<CursorFetchResult> {
        self.last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl UsageFetcher for CursorFetcher {
    type Snapshot = CursorSnapshot;

    async fn fetch(&self) -> Result<CursorSnapshot, ProviderError> {
        let cookie = self
            .session
            .cookie_header()
            .filter(|c| !c.is_empty())
            .ok_or(ProviderError::NotSignedIn(CTX))?;
        match self.client.fetch_snapshot(&cookie, self.clock.now()).await {
            Ok(result) => {
                self.session.record_auth_success();
                let snapshot = result.snapshot.clone();
                *self
                    .last
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
                Ok(snapshot)
            }
            Err(error) => {
                if matches!(error, ProviderError::Unauthorized(_)) {
                    self.session
                        .record_auth_failure("Cursor rejected the session");
                }
                Err(error)
            }
        }
    }

    fn snapshot(&self) -> Option<CursorSnapshot> {
        self.last_result().map(|r| r.snapshot)
    }
}
