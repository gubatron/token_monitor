//! REST fallback JSON parser (spec 02 section 1.2.7).
//!
//! Reads only usage-specific keys (bare `percent` / `value` are ignored) and
//! requires every percentage to lie in 0..=100 (plus rounding slack), so an
//! unrelated 200 response does not parse as usage.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::models::{
    CreditBalance, ProductCatalog, ProductUsage, WeeklyUsageSnapshot, clamp_percent,
    synthesize_products,
};

const WRAPPER_KEYS: [&str; 6] = ["usage", "data", "subscription", "billing", "credits", "result"];

/// Deepest wrapper nesting followed. The Swift parser is unbounded; the JSON
/// parser itself limits depth to 128, and this caps the work at a level no real
/// response reaches.
pub const MAX_WRAPPER_DEPTH: usize = 32;

/// Deepest `{"val": ...}` wrapper followed by [`json_number`].
const MAX_NUMBER_WRAPPER_DEPTH: usize = 8;

/// `JSON.number`: a JSON number (not a bool) or numeric string, descending
/// into `{"val":..}` / `{"value":..}`; never NaN or infinite.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use tokenmon_provider_grok::rest::json_number;
/// assert_eq!(json_number(&json!(35)), Some(35.0));
/// assert_eq!(json_number(&json!(" 4.5 ")), Some(4.5));
/// assert_eq!(json_number(&json!({"val": 7})), Some(7.0));
/// assert_eq!(json_number(&json!(true)), None);
/// ```
pub fn json_number(value: &Value) -> Option<f64> {
    number_at(value, 0)
}

fn number_at(value: &Value, depth: usize) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite()),
        Value::String(s) => s.trim_matches(' ').parse::<f64>().ok().filter(|v| v.is_finite()),
        Value::Object(map) if depth < MAX_NUMBER_WRAPPER_DEPTH => ["val", "value"]
            .iter()
            .find_map(|k| map.get(*k))
            .and_then(|inner| number_at(inner, depth + 1)),
        _ => None,
    }
}

fn first_double(map: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|k| map.get(*k).and_then(json_number))
}

fn first_string<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| map.get(*k).and_then(Value::as_str))
}

fn is_percent(value: Option<f64>) -> bool {
    value.is_none_or(|v| v.is_finite() && (0.0..=100.5).contains(&v))
}

/// ISO-8601 with or without fractional seconds.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::rest::parse_flexible_date;
/// assert!(parse_flexible_date("2026-07-13T00:00:00.000Z").is_some());
/// assert!(parse_flexible_date("2026-07-16T20:25:00Z").is_some());
/// assert!(parse_flexible_date("yesterday").is_none());
/// ```
pub fn parse_flexible_date(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Parses `data` as usage JSON; `None` when no usable pool is present.
///
/// `account_email` wins over a top-level `email` string in the payload.
/// `fetched_at` stamps the snapshot.
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use tokenmon_provider_grok::rest::parse_json;
/// let s = parse_json(br#"{"usage":{"usedPercent":42}}"#, None, Utc::now()).unwrap();
/// assert_eq!(s.used_percent, 42.0);
/// assert!(parse_json(br#"{"data":{"percent":50}}"#, None, Utc::now()).is_none());
/// ```
pub fn parse_json(
    data: &[u8],
    account_email: Option<&str>,
    fetched_at: DateTime<Utc>,
) -> Option<WeeklyUsageSnapshot> {
    let value: Value = serde_json::from_slice(data).ok()?;
    parse_any(&value, account_email, fetched_at, 0)
}

fn parse_any(
    value: &Value,
    account_email: Option<&str>,
    fetched_at: DateTime<Utc>,
    depth: usize,
) -> Option<WeeklyUsageSnapshot> {
    let dict = value.as_object()?;
    if depth < MAX_WRAPPER_DEPTH {
        let nested = WRAPPER_KEYS
            .iter()
            .filter_map(|k| dict.get(*k))
            .find_map(|inner| parse_any(inner, account_email, fetched_at, depth + 1));
        if nested.is_some() {
            return nested;
        }
    }

    let used = first_double(
        dict,
        &[
            "usedPercent",
            "usagePercent",
            "credit_usage_percent",
            "percentUsed",
            "used_percent",
            "usage_percent",
        ],
    );
    let remaining = first_double(
        dict,
        &["remainingPercent", "remaining_percent", "percentRemaining"],
    );
    if !is_percent(used) || !is_percent(remaining) {
        return None;
    }

    let products = parse_products(dict);
    let inferred = used.or_else(|| {
        let sum: f64 = products.iter().map(|p| p.percent_of_pool).sum();
        (sum > 0.0).then_some(sum)
    })?;
    if !is_percent(Some(inferred)) {
        return None;
    }
    let remaining = remaining.unwrap_or_else(|| (100.0 - inferred).max(0.0));
    let products = if products.is_empty() {
        synthesize_products(inferred)
    } else {
        products
    };
    let resets_at = first_string(
        dict,
        &[
            "resetsAt",
            "resetAt",
            "reset_at",
            "billingPeriodEnd",
            "periodEnd",
            "nextReset",
        ],
    )
    .and_then(parse_flexible_date);
    let credits = first_double(
        dict,
        &[
            "extraCredits",
            "extraCreditsBalance",
            "onDemandBalance",
            "creditsBalance",
        ],
    )
    .and_then(CreditBalance::from_f64);
    let email = account_email
        .map(str::to_owned)
        .or_else(|| dict.get("email").and_then(Value::as_str).map(str::to_owned));

    Some(
        WeeklyUsageSnapshot::new(fetched_at, inferred)
            .with_remaining(clamp_percent(remaining))
            .with_resets_at(resets_at)
            .with_products(products)
            .with_credits(credits)
            .with_account(email),
    )
}

/// Array form (`products`, `breakdown`, `productBreakdown`) first, then the map
/// form (`byProduct`, `productUsage`).
fn parse_products(dict: &Map<String, Value>) -> Vec<ProductUsage> {
    let array = ["products", "breakdown", "productBreakdown"]
        .iter()
        .find_map(|k| dict.get(*k).and_then(array_of_objects));
    if let Some(items) = array {
        return items.iter().filter_map(|item| array_product(item)).collect();
    }
    ["byProduct", "productUsage"]
        .iter()
        .find_map(|k| dict.get(*k).and_then(Value::as_object))
        .map(|map| map.iter().filter_map(|(k, v)| map_product(k, v)).collect())
        .unwrap_or_default()
}

/// The value as an array whose elements are all objects, like Swift's
/// `as? [[String: Any]]`.
fn array_of_objects(value: &Value) -> Option<Vec<&Map<String, Value>>> {
    value.as_array()?.iter().map(Value::as_object).collect()
}

fn array_product(item: &Map<String, Value>) -> Option<ProductUsage> {
    let string = |k: &str| item.get(k).and_then(Value::as_str);
    let id = string("id")
        .or_else(|| string("key"))
        .or_else(|| string("name"))
        .unwrap_or("other");
    let name = string("displayName")
        .or_else(|| string("name"))
        .or_else(|| string("label"))
        .map_or_else(|| ProductCatalog::display_name(id), str::to_owned);
    let pct = first_double(item, &["percentOfPool", "usagePercent"])?;
    Some(ProductUsage::new(&id.to_lowercase(), name, pct))
}

fn map_product(key: &str, value: &Value) -> Option<ProductUsage> {
    let pct = match value {
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite())?,
        Value::Object(nested) => first_double(nested, &["percentOfPool", "usagePercent"])?,
        _ => return None,
    };
    Some(ProductUsage::new(
        &key.to_lowercase(),
        ProductCatalog::display_name(key),
        pct,
    ))
}
