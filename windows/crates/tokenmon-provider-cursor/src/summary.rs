//! `/api/usage-summary` and `/api/auth/me` parsers (spec 02 section 3.2.1).
//!
//! Both take untrusted JSON. They never panic: wrong types are ignored, every
//! percent is clamped into `0..=100` and every money figure is finite.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tokenmon_core::{ProviderError, ProviderErrorContext};

use crate::models::{CursorPoolUsage, CursorSnapshot, PoolKind};
use tokenmon_core::usage::Percent;
use tokenmon_core::util::json::number;
use tokenmon_core::util::parse_flexible;

const CTX: ProviderErrorContext = ProviderErrorContext::Cursor;

/// `parseSummary`: bytes to a [`CursorSnapshot`] (without events data).
///
/// # Errors
/// `BadResponse("Malformed response body")` for invalid JSON and
/// `BadResponse("Expected JSON object")` for a non-object root.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_provider_cursor::parse_summary;
/// let now = Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap();
/// let body = br#"{"individualUsage":{"plan":{"used":2500,"limit":10000}}}"#;
/// let snap = parse_summary(body, now).unwrap();
/// assert_eq!(snap.used_percent, 25.0);
/// assert_eq!(snap.plan_limit_usd, Some(100.0));
/// ```
pub fn parse_summary(
    data: &[u8],
    fetched_at: DateTime<Utc>,
) -> Result<CursorSnapshot, ProviderError> {
    match serde_json::from_slice::<Value>(data) {
        Ok(Value::Object(root)) => Ok(parse_summary_object(&root, fetched_at)),
        Ok(_) => Err(ProviderError::BadResponse(
            CTX,
            "Expected JSON object".to_owned(),
        )),
        Err(_) => Err(ProviderError::BadResponse(
            CTX,
            "Malformed response body".to_owned(),
        )),
    }
}

/// Raw cents pair read from a `{used, limit}` object.
#[derive(Clone, Copy, Default)]
struct Money {
    used: Option<f64>,
    limit: Option<f64>,
}

impl Money {
    fn read(map: Option<&Map<String, Value>>) -> Self {
        map.map_or_else(Self::default, |m| Self {
            used: number(m.get("used")),
            limit: number(m.get("limit")),
        })
    }

    fn both_usd(self) -> Option<(f64, f64)> {
        Some((self.used? / 100.0, self.limit? / 100.0))
    }

    fn percent(self) -> Option<f64> {
        let limit = self.limit.filter(|l| *l > 0.0)?;
        Some(Percent::clamp(self.used.unwrap_or(0.0) / limit * 100.0))
    }
}

fn cycle_date(root: &Map<String, Value>, key: &str) -> Option<DateTime<Utc>> {
    root.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .and_then(parse_flexible)
}

/// Same as [`parse_summary`] for an already parsed root object.
pub fn parse_summary_object(
    root: &Map<String, Value>,
    fetched_at: DateTime<Utc>,
) -> CursorSnapshot {
    let individual = root.get("individualUsage").and_then(Value::as_object);
    let plan_map = individual.and_then(|i| i.get("plan").and_then(Value::as_object));
    let plan = Money::read(plan_map);
    let overall = Money::read(individual.and_then(|i| i.get("overall").and_then(Value::as_object)));
    let pooled = Money::read(
        root.get("teamUsage")
            .and_then(Value::as_object)
            .and_then(|t| t.get("pooled").and_then(Value::as_object)),
    );
    let on_demand = individual.and_then(|i| i.get("onDemand").and_then(Value::as_object));

    let auto = plan_map
        .and_then(|p| number(p.get("autoPercentUsed")))
        .map(Percent::clamp);
    let api = plan_map
        .and_then(|p| number(p.get("apiPercentUsed")))
        .map(Percent::clamp);
    let explicit_total = plan_map
        .and_then(|p| number(p.get("totalPercentUsed")))
        .map(Percent::clamp);
    let total = total_percent(explicit_total, auto, api, plan, overall, pooled);

    let cycle_end = cycle_date(root, "billingCycleEnd");
    let (plan_used_usd, plan_limit_usd) = plan_usd(plan, overall, pooled);
    CursorSnapshot {
        fetched_at,
        used_percent: total,
        pools: build_pools(total, auto, api, cycle_end),
        billing_cycle_start: cycle_date(root, "billingCycleStart"),
        billing_cycle_end: cycle_end,
        membership_type: root
            .get("membershipType")
            .and_then(Value::as_str)
            .map(str::to_owned),
        plan_used_usd,
        plan_limit_usd,
        on_demand_enabled: on_demand
            .and_then(|o| o.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        on_demand_used_usd: on_demand
            .and_then(|o| number(o.get("used")))
            .map(|c| c / 100.0),
        on_demand_limit_usd: on_demand
            .and_then(|o| number(o.get("limit")))
            .map(|c| c / 100.0),
        cost_stats: None,
        account_email: None,
        daily_tokens: BTreeMap::new(),
    }
}

fn total_percent(
    explicit: Option<f64>,
    auto: Option<f64>,
    api: Option<f64>,
    plan: Money,
    overall: Money,
    pooled: Money,
) -> f64 {
    explicit
        .or_else(|| match (auto, api) {
            (Some(a), Some(b)) => Some(Percent::clamp(a.midpoint(b))),
            _ => None,
        })
        .or(api)
        .or(auto)
        .or_else(|| plan.percent())
        .or_else(|| overall.percent())
        .or_else(|| pooled.percent())
        .unwrap_or(0.0)
}

fn plan_usd(plan: Money, overall: Money, pooled: Money) -> (Option<f64>, Option<f64>) {
    let used = plan.used.unwrap_or(0.0);
    let limit = plan.limit.unwrap_or(0.0);
    if limit > 0.0 || used > 0.0 {
        return (Some(used / 100.0), Some(limit / 100.0));
    }
    overall
        .both_usd()
        .or_else(|| pooled.both_usd())
        .map_or((None, None), |(u, l)| (Some(u), Some(l)))
}

fn build_pools(
    total: f64,
    auto: Option<f64>,
    api: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
) -> Vec<CursorPoolUsage> {
    [
        (PoolKind::Total, Some(total)),
        (PoolKind::Auto, auto),
        (PoolKind::Api, api),
    ]
    .into_iter()
    .filter_map(|(kind, used)| {
        used.map(|used_percent| CursorPoolUsage {
            kind,
            used_percent,
            resets_at,
        })
    })
    .collect()
}

/// `parseEmail`: the `email` string of a JSON object, only when it has `@`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::parse_email;
/// assert_eq!(parse_email(br#"{"email":"a@b.com"}"#).as_deref(), Some("a@b.com"));
/// assert_eq!(parse_email(br#"{"email":"nope"}"#), None);
/// assert_eq!(parse_email(b"[]"), None);
/// ```
pub fn parse_email(data: &[u8]) -> Option<String> {
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(data) else {
        return None;
    };
    map.get("email")
        .and_then(Value::as_str)
        .filter(|e| e.contains('@'))
        .map(str::to_owned)
}
