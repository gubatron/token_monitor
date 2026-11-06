//! Ports of `GrokbotUsageClientTests.swift` (spec 02 section 5.9) plus the
//! HTTP status mapping of section 2.2. Hermetic: wiremock only.

#![allow(clippy::unwrap_used, clippy::assert_is_empty)]

use chrono::{DateTime, TimeZone, Utc};
use tokenmon_core::{ProviderError, ProviderErrorContext};
use tokenmon_provider_cursor::CursorHttp;
use tokenmon_provider_grokbot::{
    GrokbotEntitlement, GrokbotSnapshot, GrokbotUsageClient, NO_ALLOWANCE_MESSAGE,
    parse_usage_status,
};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EMAIL: &str = "user@example.com";

fn fetched_at() -> DateTime<Utc> {
    Utc.timestamp_opt(1_787_000_000, 0).unwrap()
}

fn parse(body: &str) -> Result<GrokbotSnapshot, ProviderError> {
    parse_usage_status(body.as_bytes(), Some(EMAIL), fetched_at())
}

fn utc(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, 0, 0).unwrap()
}

/// Cursor-funded weekly allowance, camelCase.
#[test]
fn test_parses_cursor_funded_weekly_allowance() {
    let s = parse(
        r#"{"currentPeriodStart":"2026-08-20T11:00:00Z","nextResetTimestampUtc":"2026-08-27T11:00:00Z",
        "usagePercent":42.5,"includedLimitZero":false,"hasNonZeroIncludedLimit":true,"hasAvailableUsage":true}"#,
    )
    .unwrap();
    assert_eq!(s.used_percent, 42.5);
    assert_eq!(s.remaining_percent(), 57.5);
    assert_eq!(s.entitlement, GrokbotEntitlement::Cursor);
    assert!(s.has_included_allowance);
    assert_eq!(s.period_start, Some(utc(2026, 8, 20, 11)));
    assert_eq!(s.resets_at, Some(utc(2026, 8, 27, 11)));
    assert_eq!(s.account_email.as_deref(), Some(EMAIL));
    assert_eq!(s.fetched_at, fetched_at());
}

/// SuperGrok-funded allowance with a label.
#[test]
fn test_parses_super_grok_funded_allowance() {
    let s = parse(
        r#"{"usagePercent":10,"includedUsageSuperGrokPlan":"super_grok_heavy","grokPlanLabel":"SuperGrok Heavy"}"#,
    )
    .unwrap();
    assert_eq!(
        s.entitlement,
        GrokbotEntitlement::SuperGrok("SuperGrok Heavy".into())
    );
    assert_eq!(s.entitlement.caption_text(), "via SuperGrok Heavy");
}

/// Without a label the plan identifier is shown.
#[test]
fn test_super_grok_plan_without_label_uses_plan_identifier() {
    let s = parse(r#"{"usagePercent":10,"includedUsageSuperGrokPlan":"super_grok"}"#).unwrap();
    assert_eq!(
        s.entitlement,
        GrokbotEntitlement::SuperGrok("super_grok".into())
    );
}

/// A missing reset stays `None`; it is never substituted.
#[test]
fn test_missing_reset_yields_none_rather_than_substituted_date() {
    let s = parse(r#"{"usagePercent":5,"currentPeriodStart":"2026-08-20T11:00:00Z"}"#).unwrap();
    assert_eq!(s.resets_at, None);
    assert!(s.period_start.is_some());
}

/// snake_case proto field names are accepted.
#[test]
fn test_accepts_snake_case_proto_field_names() {
    let s = parse(
        r#"{"usage_percent":77,"current_period_start":"2026-08-20T11:00:00Z",
        "next_reset_timestamp_utc":"2026-08-27T11:00:00Z","included_limit_zero":true}"#,
    )
    .unwrap();
    assert_eq!(s.used_percent, 77.0);
    assert_eq!(s.resets_at, Some(utc(2026, 8, 27, 11)));
    assert!(!s.has_included_allowance);
}

/// `{seconds, nanos}` timestamps; `seconds: 0` counts as missing.
#[test]
fn test_parses_structured_timestamps() {
    let s = parse(
        r#"{"usagePercent":1,"nextResetTimestampUtc":{"seconds":1787000000,"nanos":500000000},
        "currentPeriodStart":{"seconds":0}}"#,
    )
    .unwrap();
    let reset = s.resets_at.unwrap();
    assert_eq!(reset.timestamp(), 1_787_000_000);
    assert_eq!(reset.timestamp_subsec_nanos(), 500_000_000);
    assert_eq!(s.period_start, None);
}

/// A login HTML page means the session is gone.
#[test]
fn test_html_body_is_treated_as_unauthorized() {
    let e = parse("<!doctype html><html><body>Sign in to continue</body></html>").unwrap_err();
    assert_eq!(
        e,
        ProviderError::Unauthorized(ProviderErrorContext::Grokbot)
    );
}

/// Other HTML is a bad response, not an auth strike.
#[test]
fn test_non_login_html_is_bad_response() {
    let e = parse("<html><body>maintenance</body></html>").unwrap_err();
    assert!(matches!(e, ProviderError::BadResponse(_, m) if m == "Unexpected HTML response"));
}

/// Non-JSON, non-HTML bytes are a malformed body.
#[test]
fn test_malformed_body_is_bad_response() {
    let e = parse("not json").unwrap_err();
    assert!(matches!(e, ProviderError::BadResponse(_, m) if m == "Malformed response body"));
}

/// `{"error":"not_authenticated"}` is unauthorized.
#[test]
fn test_not_authenticated_error_payload_is_unauthorized() {
    let e = parse(r#"{"error":"not_authenticated"}"#).unwrap_err();
    assert_eq!(
        e,
        ProviderError::Unauthorized(ProviderErrorContext::Grokbot)
    );
}

/// No percent and no allowance fields means no Bot access.
#[test]
fn test_missing_usage_percent_reports_no_bot_access() {
    let e = parse(r#"{"hasAvailableUsage":false}"#).unwrap_err();
    assert!(
        matches!(e, ProviderError::Custom { ref message, .. } if message == NO_ALLOWANCE_MESSAGE)
    );
}

/// Allowance fields without a percent mean 0% used.
#[test]
fn test_missing_usage_percent_with_allowance_fields_is_zero() {
    let s = parse(
        r#"{"currentPeriodStart":"2026-08-20T11:00:00Z","nextResetTimestampUtc":"2026-08-27T11:00:00Z",
        "hasNonZeroIncludedLimit":true}"#,
    )
    .unwrap();
    assert_eq!(s.used_percent, 0.0);
    assert!(s.has_included_allowance);
    assert!(s.resets_at.is_some());
}

/// The same with snake_case fields, even when the value is null.
#[test]
fn test_missing_usage_percent_snake_case_allowance_fields_is_zero() {
    let s = parse(r#"{"next_reset_timestamp_utc":"2026-08-27T11:00:00Z"}"#).unwrap();
    assert_eq!(s.used_percent, 0.0);
    let s = parse(r#"{"included_limit_zero":null}"#).unwrap();
    assert_eq!(s.used_percent, 0.0);
}

/// Percent is clamped to the pool bounds.
#[test]
fn test_percent_is_clamped_to_pool_bounds() {
    assert_eq!(
        parse(r#"{"usagePercent":140}"#).unwrap().used_percent,
        100.0
    );
    assert_eq!(parse(r#"{"usagePercent":-8}"#).unwrap().used_percent, 0.0);
}

/// Numeric strings and `{val}` wrappers are accepted for the percent.
#[test]
fn test_percent_accepts_numeric_string_and_wrapper() {
    assert_eq!(
        parse(r#"{"usagePercent":" 12.5 "}"#).unwrap().used_percent,
        12.5
    );
    assert_eq!(
        parse(r#"{"usagePercent":{"val":3}}"#).unwrap().used_percent,
        3.0
    );
}

/// Extreme timestamps never panic and are treated as missing.
#[test]
fn test_extreme_timestamps_are_ignored() {
    let s = parse(
        r#"{"usagePercent":1,"nextResetTimestampUtc":{"seconds":1e300,"nanos":1e300},
        "currentPeriodStart":{"seconds":9e18}}"#,
    )
    .unwrap();
    assert_eq!(s.resets_at, None);
    assert_eq!(s.period_start, None);
}

fn client_for(server: &MockServer) -> GrokbotUsageClient {
    GrokbotUsageClient::new(CursorHttp::new(&server.uri(), "TokenMon/test"))
}

async fn fetch_with(status: u16, body: &str) -> Result<GrokbotSnapshot, ProviderError> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/dashboard/get-sand-usage-status"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;
    client_for(&server)
        .fetch_usage_status("WorkosCursorSessionToken=x", Some(EMAIL), fetched_at())
        .await
}

/// The request carries the documented headers and an empty JSON object body.
#[tokio::test]
async fn test_request_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/dashboard/get-sand-usage-status"))
        .and(header("cookie", "WorkosCursorSessionToken=x"))
        .and(header("referer", "https://cursor.com/bot"))
        .and(header("origin", "https://cursor.com"))
        .and(header("user-agent", "TokenMon/test"))
        .and(body_json(serde_json::json!({})))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"usagePercent":9}"#))
        .expect(1)
        .mount(&server)
        .await;
    let s = client_for(&server)
        .fetch_usage_status("WorkosCursorSessionToken=x", None, fetched_at())
        .await
        .unwrap();
    assert_eq!(s.used_percent, 9.0);
}

/// 401 is unauthorized.
#[tokio::test]
async fn test_http_401_is_unauthorized() {
    let e = fetch_with(401, "").await.unwrap_err();
    assert_eq!(
        e,
        ProviderError::Unauthorized(ProviderErrorContext::Grokbot)
    );
}

/// A non-challenge 403 is the transient "not available" error, not unauthorized.
#[tokio::test]
async fn test_http_403_is_not_available_custom_error() {
    let e = fetch_with(403, r#"{"error":"forbidden"}"#)
        .await
        .unwrap_err();
    assert!(matches!(&e, ProviderError::Custom { message, .. }
        if message == "Grok Bot is not available on this Cursor account."));
}

/// A 403 with an HTML body is a bot challenge, a transient bad response.
#[tokio::test]
async fn test_http_403_html_is_bot_challenge() {
    let e = fetch_with(403, "<html>Just a moment...</html>")
        .await
        .unwrap_err();
    assert!(matches!(e, ProviderError::BadResponse(_, m) if m.contains("bot-protection")));
}

/// 429 is a rate-limit custom error with Grokbot wording.
#[tokio::test]
async fn test_http_429_is_rate_limited() {
    let e = fetch_with(429, "").await.unwrap_err();
    assert!(matches!(&e, ProviderError::Custom { message, .. }
        if message == "Grokbot is rate limiting requests. Retrying later."));
}

/// Other non-2xx statuses are a bad response naming the code.
#[tokio::test]
async fn test_http_500_is_bad_response() {
    let e = fetch_with(500, "").await.unwrap_err();
    assert!(matches!(e, ProviderError::BadResponse(_, m) if m == "HTTP 500"));
}

/// A refused connection is a network error with Grokbot wording.
#[tokio::test]
async fn test_transport_error_is_network() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let client = GrokbotUsageClient::new(CursorHttp::new(
        &format!("http://127.0.0.1:{port}"),
        "TokenMon/test",
    ));
    let e = client
        .fetch_usage_status("c=1", None, fetched_at())
        .await
        .unwrap_err();
    assert!(
        matches!(e, ProviderError::Network(ProviderErrorContext::Grokbot, _)),
        "{e:?}"
    );
}
