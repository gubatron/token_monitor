//! Port of `ChatGPTUsageClientTests.swift` (parse, session token, JWT claims) and
//! the window-label test from `ChatGPTAccessTokenCacheTests.swift`.
#![allow(
    clippy::unwrap_used,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]

mod common;

use common::{USAGE_BODY, jwt};
use tokenmon_core::{ProviderError, ProviderErrorContext as C};
use tokenmon_provider_chatgpt::jwt::{account_id, expiry_secs};
use tokenmon_provider_chatgpt::{UsageWindow, parse_session_token, parse_usage};

/// Spec 03 7.6 `testParseWindows`: plan, allowed, not reached, 12 and 57.5.
#[test]
fn test_parse_windows() {
    let r = parse_usage(USAGE_BODY.as_bytes()).unwrap();
    assert_eq!(r.plan_name.as_deref(), Some("plus"));
    assert!(r.allowed);
    assert!(!r.limit_reached);
    assert_eq!(r.primary.unwrap().used_percent, 12.0);
    assert_eq!(r.secondary.unwrap().used_percent, 57.5);
}

/// `testParseWindowMetadata`: window lengths and the epoch reset instant.
#[test]
fn test_parse_window_metadata() {
    let r = parse_usage(USAGE_BODY.as_bytes()).unwrap();
    let (p, s) = (r.primary.unwrap(), r.secondary.unwrap());
    assert_eq!(p.window_seconds, Some(18_000));
    assert_eq!(s.window_seconds, Some(604_800));
    assert_eq!(p.resets_at.unwrap().timestamp(), 1_787_300_000);
}

/// `testParseAlternateFieldNames`: `five_hour`/`weekly` and `utilization`.
#[test]
fn test_parse_alternate_field_names() {
    let r =
        parse_usage(br#"{"five_hour":{"used_percent":30},"weekly":{"utilization":40}}"#).unwrap();
    assert_eq!(r.primary.unwrap().used_percent, 30.0);
    assert_eq!(r.secondary.unwrap().used_percent, 40.0);
}

/// `testParseMissingWindowsYieldsNil`.
#[test]
fn test_parse_missing_windows_yields_none() {
    let r = parse_usage(br#"{"plan_type":"free"}"#).unwrap();
    assert!(r.primary.is_none() && r.secondary.is_none());
    assert_eq!(r.plan_name.as_deref(), Some("free"));
}

/// `testParseClampsOutOfRangePercent`.
#[test]
fn test_parse_clamps_out_of_range_percent() {
    let r = parse_usage(br#"{"rate_limit":{"primary_window":{"used_percent":140}}}"#).unwrap();
    assert_eq!(r.primary.unwrap().used_percent, 100.0);
}

/// `testParseInvalidPayloadThrows`, plus an HTML body and a JSON array.
#[test]
fn test_parse_invalid_payload_is_bad_response() {
    for body in [&b"nope"[..], b"<html></html>", b"[1]", b""] {
        assert_eq!(
            parse_usage(body).unwrap_err(),
            ProviderError::BadResponse(C::ChatGpt, "Unexpected usage payload".into())
        );
    }
}

/// Spec 03 section 6 item 1 (Q1): a present `null` primary blocks the
/// `five_hour` fallback, exactly like Swift; an absent key still falls back.
#[test]
fn test_quirk_null_primary_window_blocks_five_hour_fallback() {
    let blocked = parse_usage(
        br#"{"rate_limit":{"primary_window":null,"five_hour":{"used_percent":30},"secondary_window":null,"weekly":{"used_percent":40}}}"#,
    )
    .unwrap();
    assert!(blocked.primary.is_none() && blocked.secondary.is_none());
    let absent = parse_usage(br#"{"rate_limit":{"five_hour":{"used_percent":30}}}"#).unwrap();
    assert_eq!(absent.primary.unwrap().used_percent, 30.0);
}

/// Without a `rate_limit` object the root is used; `allowed` accepts 0/1
/// numbers like Swift's `as? Bool`; non-bool defaults apply.
#[test]
fn test_parse_root_fallback_and_bool_bridging() {
    let r = parse_usage(
        br#"{"rate_limit":"x","allowed":0,"limit_reached":1,"primary_window":{"used_percent":5}}"#,
    )
    .unwrap();
    assert!(!r.allowed && r.limit_reached);
    assert_eq!(r.primary.unwrap().used_percent, 5.0);
    let d = parse_usage(br#"{"allowed":2,"limit_reached":"yes"}"#).unwrap();
    assert!(d.allowed && !d.limit_reached);
}

/// `reset_at` as an ISO string; huge window seconds saturate instead of trapping.
#[test]
fn test_parse_string_reset_and_saturating_window_seconds() {
    let r = parse_usage(
        br#"{"primary_window":{"used_percent":1,"reset_at":"2026-08-21T14:00:00Z","limit_window_seconds":1e300}}"#,
    )
    .unwrap();
    let w = r.primary.unwrap();
    assert_eq!(w.resets_at.unwrap().timestamp(), 1_787_320_800);
    assert_eq!(w.window_seconds, Some(i64::MAX));
}

/// `testSessionTokenParsing`.
#[test]
fn test_session_token_parsing() {
    assert_eq!(
        parse_session_token(br#"{"accessToken":"tok-123"}"#).unwrap(),
        "tok-123"
    );
    assert_eq!(
        parse_session_token(b"{}").unwrap_err(),
        ProviderError::Unauthorized(C::ChatGpt)
    );
    assert_eq!(
        parse_session_token(br#"{"accessToken":""}"#).unwrap_err(),
        ProviderError::Unauthorized(C::ChatGpt)
    );
}

/// `testSessionTokenHTMLIsTransientNotUnauthorized`: the credential must survive.
#[test]
fn test_session_token_html_is_transient_not_unauthorized() {
    let e = parse_session_token(b"<!DOCTYPE html><html><title>Just a moment...</title></html>")
        .unwrap_err();
    assert_eq!(
        e,
        ProviderError::BadResponse(
            C::ChatGpt,
            "Sign-in page returned instead of a session token".into()
        )
    );
    assert_eq!(
        parse_session_token(b"plain").unwrap_err(),
        ProviderError::BadResponse(C::ChatGpt, "Malformed session payload".into())
    );
}

/// `testAccountIDFromAccessToken`: the misspelt `chatggpt_account_id` exercises
/// the `account_id` substring fallback (Q9); the canonical claim wins; garbage is nil.
#[test]
fn test_account_id_from_access_token() {
    let fallback = jwt(r#"{"https://api.openai.com/auth":{"chatggpt_account_id":"acct_42"}}"#);
    assert_eq!(account_id(&fallback).as_deref(), Some("acct_42"));
    let exact = jwt(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_alt"}}"#);
    assert_eq!(account_id(&exact).as_deref(), Some("acct_alt"));
    assert_eq!(account_id("garbage"), None);
}

/// Divergence D5: the fallback scan follows document order (Swift: unspecified).
#[test]
fn test_account_id_fallback_uses_document_order() {
    let t = jwt(
        r#"{"https://api.openai.com/auth":{"zz_account_id":"first","aa_account_id":"second","n":1}}"#,
    );
    assert_eq!(account_id(&t).as_deref(), Some("first"));
    let non_string_first =
        jwt(r#"{"https://api.openai.com/auth":{"zz_account_id":7,"aa_account_id":"second"}}"#);
    assert_eq!(account_id(&non_string_first).as_deref(), Some("second"));
}

/// Payload edge cases: empty segments are dropped like Swift `split`, a
/// non-object auth claim yields nil, `exp` accepts numeric strings.
#[test]
fn test_jwt_edge_cases() {
    let p = common::b64url(br#"{"exp":"1790003600","https://api.openai.com/auth":[1,2]}"#);
    assert_eq!(expiry_secs(&format!(".h.{p}.s")), Some(1_790_003_600.0));
    assert_eq!(account_id(&format!("h.{p}.s")), None);
    assert_eq!(expiry_secs("a"), None);
    assert_eq!(expiry_secs("a.!!!.c"), None);
    assert_eq!(
        expiry_secs(&format!("a.{}.c", common::b64url(b"[1]"))),
        None
    );
}

/// `testWindowLabelsDeriveFromWindowSeconds`.
#[test]
fn test_window_labels_derive_from_window_seconds() {
    let label = |s: Option<i64>| {
        UsageWindow {
            used_percent: 0.0,
            resets_at: None,
            window_seconds: s,
        }
        .label("Fallback")
    };
    assert_eq!(label(Some(18_000)), "5-Hour Window");
    assert_eq!(label(Some(3_600)), "1-Hour Window");
    assert_eq!(label(Some(86_400)), "Daily");
    assert_eq!(label(Some(604_800)), "Weekly");
    assert_eq!(label(Some(2_592_000)), "30-Day Window");
    assert_eq!(label(Some(5_400)), "Fallback");
    assert_eq!(label(None), "Fallback");
    assert_eq!(label(Some(-3_600)), "Fallback");
}
