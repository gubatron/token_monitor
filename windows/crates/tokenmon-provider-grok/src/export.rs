//! CSV and JSON export of the history (spec 02 section 1.5).

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::models::WeeklyUsageSnapshot;

/// Export file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Comma-separated values.
    Csv,
    /// Pretty-printed JSON array with sorted keys.
    Json,
}

impl ExportFormat {
    /// Default file name offered by the save dialog.
    pub const fn default_file_name(self) -> &'static str {
        match self {
            Self::Csv => "grok-usage.csv",
            Self::Json => "grok-usage.json",
        }
    }
}

/// Export failed.
#[derive(Debug, Error)]
pub enum ExportError {
    /// The JSON encoder failed.
    #[error("Could not encode the export: {0}")]
    Encode(#[from] serde_json::Error),
}

/// CSV header line.
pub const CSV_HEADER: &str =
    "fetchedAt,usedPercent,remainingPercent,resetsAt,products,extraCredits,accountEmail";

/// Encodes `snapshots` in `format`.
///
/// # Errors
///
/// [`ExportError::Encode`] when JSON encoding fails.
pub fn export(
    snapshots: &[WeeklyUsageSnapshot],
    format: ExportFormat,
) -> Result<Vec<u8>, ExportError> {
    match format {
        ExportFormat::Csv => Ok(export_csv(snapshots).into_bytes()),
        ExportFormat::Json => export_json(snapshots),
    }
}

fn iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Swift's `Double.description`: shortest round-trip digits, at least one
/// decimal for integral values, exponent form for tiny values.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::export::swift_double_description;
/// assert_eq!(swift_double_description(25.0), "25.0");
/// assert_eq!(swift_double_description(0.1), "0.1");
/// assert_eq!(swift_double_description(1e-5), "1e-05");
/// ```
pub fn swift_double_description(value: f64) -> String {
    if value == 0.0 {
        return "0.0".to_owned();
    }
    if value.abs() < 1e-4 {
        let text = format!("{value:e}");
        return match text.split_once('e') {
            Some((mantissa, exp)) => {
                let (sign, digits) = exp.strip_prefix('-').map_or(("+", exp), |d| ("-", d));
                format!("{mantissa}e{sign}{digits:0>2}")
            }
            None => text,
        };
    }
    let text = format!("{value}");
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

/// The CSV text for `snapshots`: header plus one line per row, joined with
/// `\n` and no trailing newline.
pub fn export_csv(snapshots: &[WeeklyUsageSnapshot]) -> String {
    let rows = snapshots.iter().map(csv_line);
    std::iter::once(CSV_HEADER.to_owned())
        .chain(rows)
        .collect::<Vec<_>>()
        .join("\n")
}

fn csv_line(snap: &WeeklyUsageSnapshot) -> String {
    let products = snap
        .products
        .iter()
        .map(|p| format!("{}:{}", p.id, swift_double_description(p.percent_of_pool)))
        .collect::<Vec<_>>()
        .join("|");
    let resets = snap.resets_at.map(iso).unwrap_or_default();
    let credits = snap
        .extra_credits_balance
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let email = snap.account_email.clone().unwrap_or_default();
    [
        iso(snap.fetched_at),
        format!("{:.2}", snap.used_percent),
        format!("{:.2}", snap.remaining_percent),
        csv_escape(&resets),
        csv_escape(&products),
        csv_escape(&credits),
        csv_escape(&email),
    ]
    .join(",")
}

/// Escapes one CSV cell.
///
/// A cell starting with `=`, `+`, `-`, `@`, tab or carriage return is prefixed
/// with `'` so spreadsheets read it as text; a cell containing a comma, quote,
/// line feed or carriage return is quoted with inner quotes doubled.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::export::csv_escape;
/// assert_eq!(csv_escape("-5"), "'-5");
/// assert_eq!(csv_escape("a,b"), "\"a,b\"");
/// assert_eq!(csv_escape("\r=1+1"), "\"'\r=1+1\"");
/// ```
pub fn csv_escape(value: &str) -> String {
    let sanitized = if value
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'))
    {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if sanitized.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", sanitized.replace('"', "\"\""))
    } else {
        sanitized
    }
}

fn export_json(snapshots: &[WeeklyUsageSnapshot]) -> Result<Vec<u8>, ExportError> {
    let rows: Vec<Value> = snapshots.iter().map(json_row).collect();
    Ok(serde_json::to_vec_pretty(&rows)?)
}

/// One export row with keys in sorted order (inserted from a `BTreeMap`, so
/// the order holds even if `serde_json` preserves insertion order); nil
/// values are omitted and there is no `id`.
fn json_row(snap: &WeeklyUsageSnapshot) -> Value {
    let products: Vec<Value> = snap
        .products
        .iter()
        .map(|p| {
            json!({
                "colorToken": p.color_token,
                "displayName": p.display_name,
                "id": p.id,
                "percentOfPool": p.percent_of_pool,
            })
        })
        .collect();
    let mut row = BTreeMap::new();
    row.insert("fetchedAt".into(), json!(iso(snap.fetched_at)));
    row.insert("usedPercent".into(), json!(snap.used_percent));
    row.insert("remainingPercent".into(), json!(snap.remaining_percent));
    row.insert("products".into(), Value::Array(products));
    if let Some(at) = snap.resets_at {
        row.insert("resetsAt".into(), json!(iso(at)));
    }
    if let Some(credits) = &snap.extra_credits_balance {
        row.insert("extraCreditsBalance".into(), json!(credits.value()));
    }
    if let Some(email) = &snap.account_email {
        row.insert("accountEmail".into(), json!(email));
    }
    Value::Object(row.into_iter().collect::<Map<_, _>>())
}
