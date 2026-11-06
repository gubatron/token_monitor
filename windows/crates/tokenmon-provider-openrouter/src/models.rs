//! Wire types for `/key`, `/credits` and `/activity` and their parsers.

use serde::{Deserialize, Deserializer};
use tokenmon_core::{ProviderError, ProviderErrorContext};

/// `{"data": T}` wrapper every endpoint uses.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

/// The `data` object of `GET /key`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct KeyData {
    /// Human label of the key.
    #[serde(default)]
    pub label: Option<String>,
    /// All-time spend of the key in USD (required).
    pub usage: f64,
    /// Spend today.
    #[serde(default)]
    pub usage_daily: Option<f64>,
    /// Spend this week.
    #[serde(default)]
    pub usage_weekly: Option<f64>,
    /// Spend this month.
    #[serde(default)]
    pub usage_monthly: Option<f64>,
    /// Per-key credit limit, `null` when unlimited.
    #[serde(default)]
    pub limit: Option<f64>,
    /// Credit left under the limit in the current window.
    #[serde(default)]
    pub limit_remaining: Option<f64>,
    /// Window the limit resets on (`daily`, `weekly`, `monthly`, ...).
    #[serde(default)]
    pub limit_reset: Option<String>,
    /// Free-tier account flag.
    #[serde(default)]
    pub is_free_tier: Option<bool>,
    /// Management keys may read `/credits` and `/activity`.
    #[serde(default)]
    pub is_management_key: Option<bool>,
}

/// The `data` object of `GET /credits`.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct CreditsData {
    /// Credits purchased.
    pub total_credits: f64,
    /// Credits consumed.
    pub total_usage: f64,
}

/// One row of `GET /activity`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActivityRow {
    /// Day of the row (unused by the snapshot).
    pub date: String,
    /// Model slug as reported (may be a stealth alias).
    pub model: String,
    /// Upstream provider (unused).
    #[serde(default)]
    pub provider_name: Option<String>,
    /// Reported cost in USD.
    pub usage: f64,
    /// Request count.
    #[serde(deserialize_with = "lenient_int")]
    pub requests: i64,
    /// Prompt tokens.
    #[serde(deserialize_with = "lenient_int")]
    pub prompt_tokens: i64,
    /// Completion tokens.
    #[serde(deserialize_with = "lenient_int")]
    pub completion_tokens: i64,
    /// Reasoning tokens (unused).
    #[serde(deserialize_with = "lenient_int")]
    pub reasoning_tokens: i64,
}

/// Accepts integers and integral floats (`1.0`), saturating on overflow.
fn lenient_int<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let n = serde_json::Number::deserialize(d)?;
    if let Some(i) = n.as_i64() {
        return Ok(i);
    }
    match n.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 => Ok(f as i64),
        _ => Err(serde::de::Error::custom("expected an integer")),
    }
}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ProviderError> {
    serde_json::from_slice::<Envelope<T>>(bytes)
        .map(|e| e.data)
        .map_err(|e| {
            ProviderError::BadResponse(
                ProviderErrorContext::OpenRouter,
                // Deliberately not `{e}`: serde echoes offending values
                // ("invalid type: string \"...\""), which could leak a
                // secret if a server reflects the key back.
                format!(
                    "Unexpected JSON ({:?} error at line {}, column {})",
                    e.classify(),
                    e.line(),
                    e.column()
                ),
            )
        })
}

/// Parses a `/key` body.
///
/// # Errors
/// [`ProviderError::BadResponse`] when the body is not the documented shape.
pub fn parse_key(bytes: &[u8]) -> Result<KeyData, ProviderError> {
    decode(bytes)
}

/// Parses a `/credits` body.
///
/// # Errors
/// [`ProviderError::BadResponse`] when the body is not the documented shape.
pub fn parse_credits(bytes: &[u8]) -> Result<CreditsData, ProviderError> {
    decode(bytes)
}

/// Parses an `/activity` body.
///
/// # Errors
/// [`ProviderError::BadResponse`] when the body is not the documented shape.
pub fn parse_activity(bytes: &[u8]) -> Result<Vec<ActivityRow>, ProviderError> {
    decode(bytes)
}
