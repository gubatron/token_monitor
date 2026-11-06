//! In-memory cache of the exchanged web access token (spec 03 section 2.3).

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

use crate::jwt::expiry_secs;
use crate::time::{epoch_secs, from_epoch_secs};

/// Longest a token is reused before the session endpoint is called again; the
/// exchange also renews the rolling session cookie.
pub const SESSION_RENEW_INTERVAL_SECS: i64 = 30 * 60;
/// Margin before the token's own expiry at which reuse stops.
pub const EXPIRY_MARGIN_SECS: f64 = 5.0 * 60.0;
/// Reuse period for a token whose expiry cannot be read.
pub const UNKNOWN_EXPIRY_LIFETIME_SECS: i64 = 5 * 60;

/// Reuse deadline for `access_token` fetched at `now`: `min(now + 30 min,
/// exp - 5 min)`, or `now + 5 min` when `exp` is unreadable. A token that is
/// already near expiry yields a past deadline (never reused).
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_provider_chatgpt::valid_until;
///
/// let now = Utc.timestamp_opt(1_790_000_000, 0).unwrap();
/// assert_eq!((valid_until("opaque", now) - now).num_seconds(), 300);
/// ```
pub fn valid_until(access_token: &str, now: DateTime<Utc>) -> DateTime<Utc> {
    let renew_by = now + Duration::seconds(SESSION_RENEW_INTERVAL_SECS);
    let Some(exp) = expiry_secs(access_token) else {
        return now + Duration::seconds(UNKNOWN_EXPIRY_LIFETIME_SECS);
    };
    let by_expiry = exp - EXPIRY_MARGIN_SECS;
    if by_expiry >= epoch_secs(renew_by) {
        return renew_by;
    }
    from_epoch_secs(by_expiry).unwrap_or(DateTime::<Utc>::MIN_UTC)
}

#[derive(Debug, Clone)]
struct Entry {
    key: String,
    token: String,
    valid_until: DateTime<Utc>,
}

/// Single-entry cache keyed by the session generation (stable while the cookie
/// rolls, new on sign-in/out).
#[derive(Debug, Default)]
pub struct AccessTokenCache {
    entry: Mutex<Option<Entry>>,
}

impl AccessTokenCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached token for `key` while `now < valid_until`.
    pub fn token(&self, key: &str, now: DateTime<Utc>) -> Option<String> {
        let guard = self
            .entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .as_ref()
            .filter(|e| e.key == key && now < e.valid_until)
            .map(|e| e.token.clone())
    }

    /// Replaces the entry.
    pub fn store(&self, token: &str, key: &str, valid_until: DateTime<Utc>) {
        *self
            .entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Entry {
            key: key.to_owned(),
            token: token.to_owned(),
            valid_until,
        });
    }

    /// Drops the entry.
    pub fn clear(&self) {
        *self
            .entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}
