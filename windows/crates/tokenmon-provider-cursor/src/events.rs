//! Usage-event model and the events-page parser (spec 02 section 3.2.2).
//!
//! Input is untrusted JSON. Timestamps, token counts and costs are converted
//! with saturating, non-panicking arithmetic.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tokenmon_core::{ProviderError, ProviderErrorContext};

use crate::json::{number, number_of, object_of, safe_int64};

const CTX: ProviderErrorContext = ProviderErrorContext::Cursor;

/// Models whose names start with this belong to Grokbot, not Cursor.
const BOT_MODEL_PREFIX: &str = "grok-bot";

/// Largest epoch seconds value we turn into a date (about year 318,000;
/// chrono's range ends sooner and returns `None` past it).
const MAX_EPOCH_SECONDS: f64 = 1.0e13;

/// One Cursor usage event.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorUsageEvent {
    /// When the event happened.
    pub date: DateTime<Utc>,
    /// Cents actually charged.
    pub charged_cents: f64,
    /// Request cost units (`requestsCosts`), never negative.
    pub requests_costs: f64,
    /// Input tokens.
    pub input_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Cache-read tokens.
    pub cache_read_tokens: i64,
    /// Cache-write tokens.
    pub cache_write_tokens: i64,
    /// Sum of the four token counts, computed in f64 then saturated.
    pub total_tokens: i64,
    /// Model name.
    pub model: String,
}

impl CursorUsageEvent {
    /// Parses one event; `None` for Grokbot events and events without a valid
    /// timestamp.
    pub fn from_json(json: &Map<String, Value>) -> Option<Self> {
        let model = json
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if model.starts_with(BOT_MODEL_PREFIX) {
            return None;
        }
        let date = event_timestamp(json)?;
        let usage = object_of(json, "tokenUsage");
        let count = |key: &str| usage.and_then(|u| number_of(u, key)).unwrap_or(0.0);
        let (input, output) = (count("inputTokens"), count("outputTokens"));
        let (read, write) = (count("cacheReadTokens"), count("cacheWriteTokens"));
        Some(Self {
            date,
            charged_cents: charged_cents(json, usage),
            requests_costs: number_of(json, "requestsCosts").unwrap_or(0.0).max(0.0),
            input_tokens: safe_int64(input),
            output_tokens: safe_int64(output),
            cache_read_tokens: safe_int64(read),
            cache_write_tokens: safe_int64(write),
            total_tokens: safe_int64(input + output + write + read),
            model: model.to_owned(),
        })
    }

    /// Activity weight: `requestsCosts`, else tokens, else charged cents,
    /// else 1.
    pub fn weight(&self) -> f64 {
        if self.requests_costs > 0.0 {
            self.requests_costs
        } else if self.total_tokens > 0 {
            self.total_tokens as f64
        } else if self.charged_cents > 0.0 {
            self.charged_cents
        } else {
            1.0
        }
    }
}

fn charged_cents(json: &Map<String, Value>, usage: Option<&Map<String, Value>>) -> f64 {
    number_of(json, "chargedCents")
        .filter(|c| *c > 0.0)
        .or_else(|| {
            usage
                .and_then(|u| number_of(u, "totalCents"))
                .filter(|c| *c > 0.0)
        })
        .unwrap_or(0.0)
}

/// One page of events.
#[derive(Debug, Clone, PartialEq)]
pub struct EventsPage {
    /// Parsed events (Grokbot and timestamp-less ones are dropped).
    pub events: Vec<CursorUsageEvent>,
    /// Raw number of events on the page, including skipped ones.
    pub page_count: usize,
    /// `totalUsageEventsCount`, 0 when unknown.
    pub total: i64,
}

/// `parseUsageEventsPage` for bytes.
///
/// # Errors
/// `BadResponse` for invalid JSON or a non-object root.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::parse_usage_events_page;
/// let body = br#"{"totalUsageEventsCount":"2","usageEventsDisplay":[
///   {"timestamp":"1775418973898","model":"default"},
///   {"model":"grok-bot-default","timestamp":"1775418973898"}]}"#;
/// let page = parse_usage_events_page(body).unwrap();
/// assert_eq!((page.total, page.page_count, page.events.len()), (2, 2, 1));
/// ```
pub fn parse_usage_events_page(data: &[u8]) -> Result<EventsPage, ProviderError> {
    match serde_json::from_slice::<Value>(data) {
        Ok(Value::Object(root)) => Ok(events_page_from_object(&root)),
        Ok(_) => Err(ProviderError::BadResponse(
            CTX,
            "Expected events JSON object".to_owned(),
        )),
        Err(_) => Err(ProviderError::BadResponse(
            CTX,
            "Malformed response body".to_owned(),
        )),
    }
}

/// [`parse_usage_events_page`] for an already parsed root object.
pub fn events_page_from_object(root: &Map<String, Value>) -> EventsPage {
    let total = root
        .get("totalUsageEventsCount")
        .and_then(number)
        .map_or(0, safe_int64);
    let raw: &[Value] = root
        .get("usageEventsDisplay")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    EventsPage {
        events: raw
            .iter()
            .filter_map(Value::as_object)
            .filter_map(CursorUsageEvent::from_json)
            .collect(),
        page_count: raw.len(),
        total,
    }
}

/// `eventTimestamp`: the `timestamp` field as a string, double or int epoch.
pub fn event_timestamp(event: &Map<String, Value>) -> Option<DateTime<Utc>> {
    let raw = match event.get("timestamp")? {
        Value::String(s) => s.parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    date_from_epoch_number(raw)
}

/// `date(fromEpochNumber:)`: finite and positive; above 1e12 means
/// milliseconds. Values beyond the representable calendar give `None`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_cursor::date_from_epoch_number;
/// assert_eq!(date_from_epoch_number(1775418973898.0), date_from_epoch_number(1775418973.898));
/// assert!(date_from_epoch_number(0.0).is_none());
/// assert!(date_from_epoch_number(-5.0).is_none());
/// assert!(date_from_epoch_number(1e300).is_none());
/// ```
pub fn date_from_epoch_number(value: f64) -> Option<DateTime<Utc>> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let seconds = if value > 1.0e12 {
        value / 1000.0
    } else {
        value
    };
    if seconds > MAX_EPOCH_SECONDS {
        return None;
    }
    let whole = seconds.floor();
    let nanos = (((seconds - whole) * 1.0e9) as u32).min(999_999_999);
    DateTime::from_timestamp(whole as i64, nanos)
}
