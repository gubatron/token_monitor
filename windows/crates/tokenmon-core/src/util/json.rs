//! Helpers for pulling values out of decoded JSON trees.
//!
//! Keeps numeric coercion, key-order fallback and nested traversal consistent
//! across providers. All functions are total: no input panics.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Map, Value};

/// Trims spaces, tabs and other horizontal whitespace (not newlines), like Foundation's `.whitespaces`.
fn trim_horizontal(s: &str) -> &str {
    s.trim_matches(|c: char| {
        c.is_whitespace()
            && !matches!(
                c,
                '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
            )
    })
}

/// Coerces a JSON value to a finite `f64`, descending into `{"val": ..}` /
/// `{"value": ..}` wrappers. Booleans, NaN and infinities give `None`; strings
/// are trimmed and parsed.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use tokenmon_core::util::json::number;
/// assert_eq!(number(Some(&json!(" 1.5 "))), Some(1.5));
/// assert_eq!(number(Some(&json!({"val": "0.1"}))), Some(0.1));
/// assert_eq!(number(Some(&json!(true))), None);
/// assert_eq!(number(Some(&json!("1e400"))), None);
/// ```
#[must_use]
pub fn number(v: Option<&Value>) -> Option<f64> {
    let value = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => trim_horizontal(s).parse::<f64>().ok()?,
        Value::Object(o) => return number(o.get("val")).or_else(|| number(o.get("value"))),
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn parse_decimal(text: &str) -> Option<Decimal> {
    Decimal::from_str(text)
        .or_else(|_| Decimal::from_scientific(text))
        .ok()
}

/// Coerces a JSON value to a [`Decimal`] without a float round trip for strings,
/// so money keeps its written digits (`"19.99"` stays exactly 19.99). Same
/// acceptance rules as [`number`]; a string must first parse as a finite double.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use tokenmon_core::util::json::decimal;
/// assert_eq!(decimal(Some(&json!("19.99"))).unwrap().to_string(), "19.99");
/// assert!(decimal(Some(&json!("1.5abc"))).is_none());
/// ```
#[must_use]
pub fn decimal(v: Option<&Value>) -> Option<Decimal> {
    match v? {
        Value::Number(n) => {
            n.as_f64().filter(|f| f.is_finite())?;
            parse_decimal(&n.to_string())
        }
        Value::String(s) => {
            let trimmed = trim_horizontal(s);
            trimmed.parse::<f64>().ok().filter(|f| f.is_finite())?;
            parse_decimal(trimmed)
        }
        Value::Object(o) => decimal(o.get("val")).or_else(|| decimal(o.get("value"))),
        _ => None,
    }
}

/// The string value, only for JSON strings.
#[must_use]
pub fn string(v: Option<&Value>) -> Option<&str> {
    v?.as_str()
}

/// Walks nested objects along `keys`; `None` if any step is not an object or is missing.
#[must_use]
pub fn nested<'a>(dict: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    let (first, rest) = keys.split_first()?;
    rest.iter()
        .try_fold(dict.get(*first)?, |cur, k| cur.as_object()?.get(*k))
}

/// The first numeric value among `keys`, in order.
#[must_use]
pub fn first_double(dict: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| number(dict.get(*k)))
}

/// The first string among `keys`, in order.
#[must_use]
pub fn first_string<'a>(dict: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| string(dict.get(*k)))
}

/// The first [`Decimal`] among `keys`, in order.
#[must_use]
pub fn first_decimal(dict: &Map<String, Value>, keys: &[&str]) -> Option<Decimal> {
    keys.iter().find_map(|k| decimal(dict.get(*k)))
}

/// The first boolean among `keys`, in order. A numeric `0`/`1` is accepted
/// because some protobuf-JSON encodings emit flags as numbers.
#[must_use]
pub fn first_bool(dict: &Map<String, Value>, keys: &[&str], fallback: bool) -> bool {
    for k in keys {
        if let Some(Value::Bool(b)) = dict.get(*k) {
            return *b;
        }
        if let Some(n) = number(dict.get(*k)) {
            return n != 0.0;
        }
    }
    fallback
}
