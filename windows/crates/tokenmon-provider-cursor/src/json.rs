//! Lenient JSON readers (`JSON.number`, `firstDouble`, `firstString`,
//! `firstBool`, `ISO8601.parseFlexible`) and the saturating integer casts.
//!
//! Private stand-ins for the ws-core-usage `util` helpers, public so Grokbot
//! (same payload family) can reuse them until core exports its own.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

/// Deepest `{"val":...}` wrapper chain followed by [`number`].
const MAX_WRAPPER_DEPTH: usize = 16;

/// `JSON.number`: a JSON number (never a bool) or a numeric string (trimmed),
/// looking through `{"val":..}` / `{"value":..}` wrappers; finite only.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use tokenmon_provider_cursor::json::number;
/// assert_eq!(number(&json!(" 12.5 ")), Some(12.5));
/// assert_eq!(number(&json!({"val": 3})), Some(3.0));
/// assert_eq!(number(&json!(true)), None);
/// assert_eq!(number(&json!("inf")), None);
/// ```
pub fn number(value: &Value) -> Option<f64> {
    number_at(value, 0)
}

fn number_at(value: &Value, depth: usize) -> Option<f64> {
    let parsed = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Object(map) if depth < MAX_WRAPPER_DEPTH => ["val", "value"]
            .iter()
            .find_map(|k| map.get(*k))
            .and_then(|inner| number_at(inner, depth + 1)),
        _ => None,
    };
    parsed.filter(|v| v.is_finite())
}

/// [`number`] of `map[key]`.
pub fn number_of(map: &Map<String, Value>, key: &str) -> Option<f64> {
    map.get(key).and_then(number)
}

/// The first key whose value passes [`number`].
pub fn first_double(map: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| number_of(map, k))
}

/// The first key holding a JSON string (no coercion).
pub fn first_string<'a>(map: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| map.get(*k).and_then(Value::as_str))
}

/// The first key holding a bool, or a number (`!= 0`); else `fallback`.
pub fn first_bool(map: &Map<String, Value>, keys: &[&str], fallback: bool) -> bool {
    keys.iter()
        .find_map(|k| match map.get(*k)? {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => n.as_f64().map(|v| v != 0.0),
            _ => None,
        })
        .unwrap_or(fallback)
}

/// A sub-object, or an empty map when missing or of another type.
pub fn object_of<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    map.get(key).and_then(Value::as_object)
}

/// `ISO8601.parseFlexible`: RFC 3339 with or without fractional seconds.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::json::parse_flexible;
/// assert!(parse_flexible("2026-07-13T00:00:00.000Z").is_some());
/// assert!(parse_flexible("2026-07-16T20:25:00Z").is_some());
/// assert!(parse_flexible("yesterday").is_none());
/// ```
pub fn parse_flexible(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// `safeInt64`: non-finite gives 0, out-of-range saturates, else truncates.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::json::safe_int64;
/// assert_eq!(safe_int64(1e30), i64::MAX);
/// assert_eq!(safe_int64(-1e30), i64::MIN);
/// assert_eq!(safe_int64(f64::NAN), 0);
/// assert_eq!(safe_int64(f64::INFINITY), 0);
/// assert_eq!(safe_int64(12.9), 12);
/// ```
pub fn safe_int64(value: f64) -> i64 {
    if !value.is_finite() {
        0
    } else if value >= i64::MAX as f64 {
        i64::MAX
    } else if value <= i64::MIN as f64 {
        i64::MIN
    } else {
        value as i64
    }
}

/// `Percent.clamp`: into `0..=100`; NaN gives 0.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::json::clamp_percent;
/// assert_eq!(clamp_percent(120.0), 100.0);
/// assert_eq!(clamp_percent(-3.0), 0.0);
/// assert_eq!(clamp_percent(f64::NAN), 0.0);
/// ```
pub fn clamp_percent(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 100.0)
    }
}
