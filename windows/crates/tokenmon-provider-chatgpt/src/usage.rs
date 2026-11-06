//! `wham/usage` parsing and the snapshot model (spec 03 section 2.5).

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tokenmon_core::net::{ProviderError, ProviderErrorContext};
use tokenmon_core::usage::{Percent, UsagePool};
use tokenmon_core::util::json::{first_double, number, string};
use tokenmon_core::util::parse_flexible;

use crate::time::from_epoch_secs;

/// One Codex rate-limit window.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// 0..=100 used (the endpoint can report more than 100 at the edge).
    pub used_percent: f64,
    /// When the window resets.
    pub resets_at: Option<DateTime<Utc>>,
    /// Window length from `limit_window_seconds`, truncated; out-of-range
    /// values saturate (Swift's `Int(Double)` would trap).
    pub window_seconds: Option<i64>,
}

impl UsageWindow {
    /// `remainingPercent`.
    pub fn remaining_percent(&self) -> f64 {
        Percent::clamp(100.0 - self.used_percent)
    }

    /// Track label derived from the window length, or `fallback`.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_provider_chatgpt::UsageWindow;
    /// let w = |s| UsageWindow { used_percent: 1.0, resets_at: None, window_seconds: s };
    /// assert_eq!(w(Some(18_000)).label("x"), "5-Hour Window");
    /// assert_eq!(w(Some(604_800)).label("x"), "Weekly");
    /// assert_eq!(w(Some(2_592_000)).label("x"), "30-Day Window");
    /// assert_eq!(w(Some(5_400)).label("x"), "x");
    /// assert_eq!(w(None).label("x"), "x");
    /// ```
    pub fn label(&self, fallback: &str) -> String {
        let Some(seconds) = self.window_seconds.filter(|s| *s > 0) else {
            return fallback.to_owned();
        };
        // Integer form of the Swift `hours` switch (exact for every i64).
        if seconds == 168 * 3600 {
            "Weekly".to_owned()
        } else if seconds == 24 * 3600 {
            "Daily".to_owned()
        } else if seconds < 24 * 3600 && seconds % 3600 == 0 {
            format!("{}-Hour Window", seconds / 3600)
        } else if seconds % (24 * 3600) == 0 {
            format!("{}-Day Window", seconds / (24 * 3600))
        } else {
            fallback.to_owned()
        }
    }
}

/// Parsed usage payload.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageResponse {
    /// `plan_type` of the root object.
    pub plan_name: Option<String>,
    /// `allowed`, default true.
    pub allowed: bool,
    /// `limit_reached`, default false.
    pub limit_reached: bool,
    /// 5-hour window.
    pub primary: Option<UsageWindow>,
    /// Weekly window.
    pub secondary: Option<UsageWindow>,
}

/// Swift `as? Bool` on a JSON value: booleans, and numbers equal to 0 or 1
/// (NSNumber bridging).
fn bridged_bool(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => match n.as_f64()? {
            0.0 => Some(false),
            1.0 => Some(true),
            _ => None,
        },
        _ => None,
    }
}

fn window(raw: Option<&Value>) -> Option<UsageWindow> {
    let dict: &Map<String, Value> = raw?.as_object()?;
    let used = first_double(dict, &["used_percent", "utilization"])?;
    let resets_at = dict.get("reset_at").and_then(|v| {
        number(Some(v)).map_or_else(|| string(Some(v)).and_then(parse_flexible), from_epoch_secs)
    });
    Some(UsageWindow {
        used_percent: Percent::clamp(used),
        resets_at,
        window_seconds: first_double(dict, &["limit_window_seconds"]).map(|s| s as i64),
    })
}

/// `ChatGPTUsageResponse.parse`.
///
/// Parity quirk (spec 03 section 6 item 1): a present-but-`null`
/// `primary_window` does **not** fall back to `five_hour` (same for
/// `secondary_window` and `weekly`), exactly like Swift's `??` on `NSNull`.
///
/// # Errors
///
/// `BadResponse("Unexpected usage payload")` unless the body is a JSON object.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::parse_usage;
/// let r = parse_usage(br#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":12}}}"#).unwrap();
/// assert_eq!(r.primary.unwrap().used_percent, 12.0);
/// assert!(parse_usage(b"nope").is_err());
/// ```
pub fn parse_usage(data: &[u8]) -> Result<UsageResponse, ProviderError> {
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(data) else {
        return Err(ProviderError::BadResponse(
            ProviderErrorContext::ChatGpt,
            "Unexpected usage payload".into(),
        ));
    };
    let rl = root
        .get("rate_limit")
        .and_then(Value::as_object)
        .unwrap_or(&root);
    Ok(UsageResponse {
        plan_name: string(root.get("plan_type")).map(str::to_owned),
        allowed: bridged_bool(rl.get("allowed")).unwrap_or(true),
        limit_reached: bridged_bool(rl.get("limit_reached")).unwrap_or(false),
        primary: window(rl.get("primary_window").or_else(|| rl.get("five_hour"))),
        secondary: window(rl.get("secondary_window").or_else(|| rl.get("weekly"))),
    })
}

/// Headline Codex rate-limit snapshot (5-hour and weekly windows).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatGptSnapshot {
    /// When the data was fetched.
    pub fetched_at: DateTime<Utc>,
    /// Raw plan name.
    pub plan_name: Option<String>,
    /// Whether Codex access is allowed.
    pub allowed: bool,
    /// Whether the limit was reached.
    pub limit_reached: bool,
    /// 5-hour window.
    pub primary: Option<UsageWindow>,
    /// Weekly window.
    pub secondary: Option<UsageWindow>,
}

impl ChatGptSnapshot {
    /// Builds a snapshot from a parsed response.
    pub fn new(response: UsageResponse, fetched_at: DateTime<Utc>) -> Self {
        Self {
            fetched_at,
            plan_name: response.plan_name,
            allowed: response.allowed,
            limit_reached: response.limit_reached,
            primary: response.primary,
            secondary: response.secondary,
        }
    }

    /// `primary.resetsAt ?? secondary.resetsAt`.
    pub fn resets_at(&self) -> Option<DateTime<Utc>> {
        self.primary
            .as_ref()
            .and_then(|w| w.resets_at)
            .or_else(|| self.secondary.as_ref().and_then(|w| w.resets_at))
    }

    /// `(primary ?? secondary).usedPercent ?? 0`.
    pub fn headline_used_percent(&self) -> f64 {
        self.primary
            .as_ref()
            .or(self.secondary.as_ref())
            .map_or(0.0, |w| w.used_percent)
    }

    /// `ChatGPT` when the plan is missing or empty, else `Plus`-style casing.
    pub fn display_plan_name(&self) -> String {
        match self.plan_name.as_deref().filter(|p| !p.is_empty()) {
            None => "ChatGPT".to_owned(),
            Some(plan) => {
                let mut chars = plan.chars();
                let first: String = chars
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
                format!("{first}{}", chars.as_str().to_lowercase())
            }
        }
    }

    /// Five-hour primary plus weekly secondary pool.
    pub fn usage_pool(&self) -> UsagePool {
        let pools: Vec<UsagePool> = [
            self.primary.as_ref().map(|_| UsagePool::Hourly),
            self.secondary.as_ref().map(|_| UsagePool::Weekly),
        ]
        .into_iter()
        .flatten()
        .collect();
        UsagePool::combining(&pools)
    }
}
