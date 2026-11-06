//! Contract tests for errors and clocks.

use std::time::Duration;

use chrono::{FixedOffset, TimeZone, Utc};
use tokenmon_core::{
    Clock, ManualClock, ProviderError, ProviderErrorContext as Ctx, SystemClock, UsageError,
};

/// `UsageError` descriptions match spec 01 section 1.1.4.
#[test]
fn test_usage_error_messages() {
    assert_eq!(
        UsageError::NotSignedIn.to_string(),
        "Sign in to load usage."
    );
    assert_eq!(
        UsageError::Unauthorized.to_string(),
        "Session expired. Sign in again."
    );
    assert_eq!(
        UsageError::Network("x".into()).to_string(),
        "Network error: x"
    );
    assert_eq!(
        UsageError::BadResponse("y".into()).to_string(),
        "Response error: y"
    );
    assert_eq!(
        UsageError::RateLimited { retry_after: None }.to_string(),
        "Rate limited. Retrying later."
    );
}

/// `retry_after` only exists for sane rate-limit hints.
#[test]
fn test_retry_after_only_for_rate_limited() {
    let rl = |v| UsageError::RateLimited { retry_after: v };
    assert_eq!(
        rl(Some(2.5)).retry_after(),
        Some(Duration::from_millis(2500))
    );
    assert_eq!(rl(None).retry_after(), None);
    assert_eq!(rl(Some(-1.0)).retry_after(), None);
    assert_eq!(rl(Some(f64::NAN)).retry_after(), None);
    assert_eq!(UsageError::Unauthorized.retry_after(), None);
}

/// Every context string from the spec table.
#[test]
fn test_provider_context_strings() {
    let rows = [
        (
            Ctx::Claude,
            "Claude",
            "Sign in to Claude to load usage.",
            "Claude session expired. Sign in again.",
        ),
        (
            Ctx::ChatGpt,
            "ChatGPT",
            "Sign in to ChatGPT to load usage.",
            "ChatGPT session expired. Sign in again.",
        ),
        (
            Ctx::Cursor,
            "Cursor",
            "Sign in to Cursor to load usage.",
            "Cursor session expired. Sign in again.",
        ),
        (
            Ctx::Grok,
            "Grok",
            "Sign in to grok.com to load usage.",
            "Session expired. Please sign in again.",
        ),
        (
            Ctx::Grokbot,
            "Grokbot",
            "Sign in to Cursor to load your Grokbot allowance.",
            "Cursor session expired. Sign in again.",
        ),
        (
            Ctx::OpenCode,
            "OpenCode console",
            "Sign in to the OpenCode console to load official Go usage.",
            "OpenCode console session expired. Sign in again.",
        ),
        (
            Ctx::OpenRouter,
            "OpenRouter",
            "Add an OpenRouter API key to load usage.",
            "OpenRouter rejected the API key. Check or replace it.",
        ),
    ];
    for (ctx, name, nsi, unauth) in rows {
        assert_eq!(ctx.display_name(), name);
        assert_eq!(ctx.not_signed_in_message(), nsi);
        assert_eq!(ctx.unauthorized_message(), unauth);
        assert_eq!(ProviderError::NotSignedIn(ctx).to_string(), nsi);
        assert_eq!(ProviderError::Unauthorized(ctx).to_string(), unauth);
    }
    assert_eq!(
        Ctx::Grokbot.forbidden_message(),
        Some("Grok Bot is not available on this Cursor account.")
    );
    assert_eq!(Ctx::Claude.forbidden_message(), None);
}

/// `ProviderError` descriptions and the `usage_error` mapping.
#[test]
fn test_provider_error_display_and_usage_error() {
    let net = ProviderError::Network(Ctx::Cursor, "boom".into());
    assert_eq!(net.to_string(), "Cursor network error: boom");
    assert_eq!(net.usage_error(), UsageError::Network("boom".into()));
    let bad = ProviderError::BadResponse(Ctx::OpenCode, "bad".into());
    assert_eq!(bad.to_string(), "OpenCode console response error: bad");
    let custom = ProviderError::Custom {
        message: "Claude is rate limiting requests. Retrying later.".into(),
        usage: UsageError::RateLimited {
            retry_after: Some(7.0),
        },
    };
    assert_eq!(
        custom.to_string(),
        "Claude is rate limiting requests. Retrying later."
    );
    assert_eq!(
        custom.usage_error().retry_after(),
        Some(Duration::from_secs(7))
    );
}

/// `ManualClock` only moves when told to and honours its offset.
#[test]
fn test_manual_clock_advance_set_and_offset() {
    let start = Utc.with_ymd_and_hms(2026, 3, 1, 23, 30, 0).unwrap();
    let clock = ManualClock::new(start).with_offset(FixedOffset::east_opt(3600).unwrap());
    assert_eq!(clock.now(), start);
    clock.advance(Duration::from_mins(30));
    assert_eq!(clock.now(), start + chrono::Duration::minutes(30));
    assert_eq!(clock.now_local().to_rfc3339(), "2026-03-02T01:00:00+01:00");
    clock.set(start);
    assert_eq!(clock.now(), start);
    assert_eq!(ManualClock::from_unix(0).now().timestamp(), 0);
}

/// The system clock is monotonic enough and produces a valid offset.
#[test]
fn test_system_clock_is_sane() {
    let a = SystemClock.now();
    let b = SystemClock.now();
    assert!(b >= a);
    let off = SystemClock.local_offset_at(a).local_minus_utc();
    assert!((-14 * 3600..=14 * 3600).contains(&off));
}
