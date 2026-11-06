//! Shared fixtures for the Cursor integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::needless_pass_by_value)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};
use tokenmon_core::{ManualClock, ProviderError};
use tokenmon_provider_cursor::{CursorHttp, CursorUsageClient, CursorUsageEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// UTC instant helper.
pub fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

/// Builds one event from loosely typed JSON, as the server would send it.
pub fn event(value: Value) -> CursorUsageEvent {
    CursorUsageEvent::from_json(value.as_object().unwrap()).expect("valid event")
}

/// An event at `when` with the given model, charged cents and token counts.
pub fn event_at(
    when: DateTime<Utc>,
    model: &str,
    cents: f64,
    input: i64,
    output: i64,
) -> CursorUsageEvent {
    event(json!({
        "timestamp": when.timestamp_millis().to_string(),
        "model": model,
        "chargedCents": cents,
        "tokenUsage": {"inputTokens": input, "outputTokens": output},
    }))
}

/// Summary fixture: cycle `now - 5d .. now + 25d`, plan 100/2000 cents, 5%.
pub fn summary_body(now: DateTime<Utc>) -> Value {
    json!({
        "billingCycleStart": (now - chrono::Duration::days(5)).to_rfc3339(),
        "billingCycleEnd": (now + chrono::Duration::days(25)).to_rfc3339(),
        "membershipType": "pro",
        "individualUsage": {"plan": {"used": 100, "limit": 2000, "totalPercentUsed": 5}},
    })
}

/// Answers the events endpoint from an in-memory list, honoring `startDate`.
pub struct EventsResponder {
    /// `(timestamp ms, charged cents)` of events the server knows.
    pub events: Vec<(i64, i64)>,
    /// When set, answers 500.
    pub fail: Arc<AtomicBool>,
}

impl Respond for EventsResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if self.fail.load(Ordering::SeqCst) {
            return ResponseTemplate::new(500);
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let start: i64 = body["startDate"].as_str().unwrap().parse().unwrap();
        let shown: Vec<Value> = self
            .events
            .iter()
            .filter(|(ms, _)| *ms >= start)
            .map(|(ms, cents)| {
                json!({
                    "timestamp": ms.to_string(),
                    "model": "default",
                    "chargedCents": cents,
                    "tokenUsage": {"inputTokens": 10, "outputTokens": 5},
                })
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "totalUsageEventsCount": shown.len(),
            "usageEventsDisplay": shown,
        }))
    }
}

/// A mock Cursor server plus a client and clock wired to it.
pub struct Harness {
    pub server: MockServer,
    pub client: CursorUsageClient,
    pub clock: Arc<ManualClock>,
    pub fail_events: Arc<AtomicBool>,
}

/// Starts a server serving the summary fixture for `now` and `events`.
pub async fn harness(now: DateTime<Utc>, events: Vec<(i64, i64)>) -> Harness {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/usage-summary"))
        .respond_with(ResponseTemplate::new(200).set_body_json(summary_body(now)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"email": "a@b.com"})))
        .mount(&server)
        .await;
    let fail_events = Arc::new(AtomicBool::new(false));
    Mock::given(method("POST"))
        .and(path("/api/dashboard/get-filtered-usage-events"))
        .respond_with(EventsResponder {
            events,
            fail: fail_events.clone(),
        })
        .mount(&server)
        .await;
    let clock = Arc::new(ManualClock::new(now));
    let http = CursorHttp::new(&server.uri(), "TokenMon/test");
    let client = CursorUsageClient::new(http, clock.clone());
    Harness {
        server,
        client,
        clock,
        fail_events,
    }
}

impl Harness {
    /// Number of requests received for `path`.
    pub async fn count(&self, route: &str) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == route)
            .count()
    }

    /// `startDate` (ms) of every events request, in order.
    pub async fn event_starts(&self) -> Vec<i64> {
        self.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/api/dashboard/get-filtered-usage-events")
            .map(|r| {
                let v: Value = serde_json::from_slice(&r.body).unwrap();
                v["startDate"].as_str().unwrap().parse().unwrap()
            })
            .collect()
    }
}

/// Whether an error is `Unauthorized`.
pub fn is_unauthorized(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Unauthorized(_))
}
