//! `parseUsageStatus` (spec 02 section 2.3): the parser for the
//! `get-sand-usage-status` body. It accepts camelCase and snake_case field
//! names and never panics on any input.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tokenmon_core::{ProviderError, ProviderErrorContext, UsageError};
use tokenmon_provider_cursor::json::{
    clamp_percent, first_bool, first_double, first_string, number, parse_flexible,
};
use tokenmon_provider_cursor::reject_unauthorized_body;

use crate::models::{GrokbotEntitlement, GrokbotSnapshot};

const CTX: ProviderErrorContext = ProviderErrorContext::Grokbot;

/// Message when the account has no Grok Bot allowance.
pub const NO_ALLOWANCE_MESSAGE: &str = "No Grok Bot allowance on this account. Grokbot needs a Cursor or SuperGrok plan that includes it.";

const PERCENT_KEYS: [&str; 2] = ["usagePercent", "usage_percent"];
const START_KEYS: [&str; 2] = ["currentPeriodStart", "current_period_start"];
const RESET_KEYS: [&str; 2] = ["nextResetTimestampUtc", "next_reset_timestamp_utc"];
const LIMIT_ZERO_KEYS: [&str; 2] = ["includedLimitZero", "included_limit_zero"];
const NON_ZERO_KEYS: [&str; 2] = ["hasNonZeroIncludedLimit", "has_non_zero_included_limit"];
const PLAN_KEYS: [&str; 2] = [
    "includedUsageSuperGrokPlan",
    "included_usage_super_grok_plan",
];
const LABEL_KEYS: [&str; 2] = ["grokPlanLabel", "grok_plan_label"];

/// Largest absolute epoch second accepted from a structured timestamp.
const MAX_EPOCH_SECONDS: f64 = 1.0e12;

/// "Present" means the key exists, even with a null value.
fn has_allowance_field(root: &Map<String, Value>) -> bool {
    [
        &START_KEYS,
        &RESET_KEYS,
        &LIMIT_ZERO_KEYS,
        &NON_ZERO_KEYS,
        &PLAN_KEYS,
        &LABEL_KEYS,
    ]
    .iter()
    .flat_map(|keys| keys.iter())
    .any(|k| root.contains_key(*k))
}

/// A `{seconds, nanos}` object as an instant; `seconds <= 0` counts as missing.
fn structured_timestamp(map: &Map<String, Value>) -> Option<DateTime<Utc>> {
    let seconds = map.get("seconds").and_then(number).filter(|s| *s > 0.0)?;
    let nanos = map.get("nanos").and_then(number).unwrap_or(0.0);
    let total = seconds + nanos / 1e9;
    if !total.is_finite() || total.abs() > MAX_EPOCH_SECONDS {
        return None;
    }
    let whole = total.floor();
    let frac_nanos = ((total - whole) * 1e9).round().clamp(0.0, 999_999_999.0);
    DateTime::from_timestamp(whole as i64, frac_nanos as u32)
}

/// The first key whose value is an ISO string or a `{seconds, nanos}` object.
fn first_date(root: &Map<String, Value>, keys: &[&str]) -> Option<DateTime<Utc>> {
    keys.iter().find_map(|key| match root.get(*key)? {
        Value::String(text) => parse_flexible(text),
        Value::Object(map) => structured_timestamp(map),
        _ => None,
    })
}

fn entitlement(root: &Map<String, Value>) -> GrokbotEntitlement {
    let plan = first_string(root, &PLAN_KEYS)
        .map(str::trim)
        .unwrap_or_default();
    if plan.is_empty() {
        return GrokbotEntitlement::Cursor;
    }
    let label = first_string(root, &LABEL_KEYS)
        .map(str::trim)
        .unwrap_or_default();
    GrokbotEntitlement::SuperGrok(if label.is_empty() { plan } else { label }.to_owned())
}

fn percent_of(root: &Map<String, Value>) -> Result<f64, ProviderError> {
    match first_double(root, &PERCENT_KEYS) {
        Some(p) => Ok(p),
        None if has_allowance_field(root) => Ok(0.0),
        None => Err(ProviderError::Custom {
            message: NO_ALLOWANCE_MESSAGE.to_owned(),
            usage: UsageError::BadResponse(NO_ALLOWANCE_MESSAGE.to_owned()),
        }),
    }
}

/// Parses a `get-sand-usage-status` response body.
///
/// # Errors
/// Only [`ProviderError::Unauthorized`] (login page or `not_authenticated`),
/// [`ProviderError::BadResponse`] (other HTML, malformed body) and
/// [`ProviderError::Custom`] (no allowance) are produced.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_provider_grokbot::{parse_usage_status, GrokbotEntitlement};
/// let now = Utc.with_ymd_and_hms(2026, 8, 25, 0, 0, 0).unwrap();
/// let s = parse_usage_status(br#"{"usage_percent": 77}"#, None, now).unwrap();
/// assert_eq!(s.used_percent, 77.0);
/// assert_eq!(s.entitlement, GrokbotEntitlement::Cursor);
/// ```
pub fn parse_usage_status(
    body: &[u8],
    account_email: Option<&str>,
    fetched_at: DateTime<Utc>,
) -> Result<GrokbotSnapshot, ProviderError> {
    let root = reject_unauthorized_body(body, CTX)?;
    let percent = percent_of(&root)?;
    let has_included_allowance =
        !first_bool(&root, &LIMIT_ZERO_KEYS, false) && first_bool(&root, &NON_ZERO_KEYS, true);
    Ok(GrokbotSnapshot {
        fetched_at,
        used_percent: clamp_percent(percent),
        period_start: first_date(&root, &START_KEYS),
        resets_at: first_date(&root, &RESET_KEYS),
        entitlement: entitlement(&root),
        account_email: account_email.map(str::to_owned),
        has_included_allowance,
    })
}
