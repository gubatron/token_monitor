//! Port of `CursorEventCacheTests.swift` (wiremock, injected clock).

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{at, harness, is_unauthorized};
use tokenmon_core::Clock;
use tokenmon_provider_cursor::{
    CursorCacheEntry, CursorEventCache, EVENTS_MAX_PAGES, events_window_start,
};

const COOKIE: &str = "WorkosCursorSessionToken=t";
const SUMMARY: &str = "/api/usage-summary";
const EVENTS: &str = "/api/dashboard/get-filtered-usage-events";

fn now() -> chrono::DateTime<chrono::Utc> {
    at(2026, 8, 10, 12, 0)
}

fn one_event(cents: i64) -> Vec<(i64, i64)> {
    vec![(
        (now() - chrono::Duration::hours(1)).timestamp_millis(),
        cents,
    )]
}

/// A cycle start inside the cap is returned as-is.
#[test]
fn test_events_window_start_is_cycle_start_within_cap() {
    let clock = tokenmon_core::ManualClock::new(now());
    let start = now() - chrono::Duration::days(10);
    assert_eq!(events_window_start(&clock, Some(start), now()), start);
}

/// A far (or unknown) cycle start is capped at local midnight 31 days ago.
#[test]
fn test_events_window_start_caps_at_thirty_one_days() {
    let clock = tokenmon_core::ManualClock::new(now());
    let cap = at(2026, 7, 10, 0, 0);
    let far = now() - chrono::Duration::days(90);
    assert_eq!(events_window_start(&clock, Some(far), now()), cap);
    assert_eq!(events_window_start(&clock, None, now()), cap);
}

/// Within five minutes events are not paged again; after that they are.
#[tokio::test]
async fn test_events_are_paged_once_within_refresh_interval() {
    let h = harness(now(), one_event(250)).await;
    let first = h.client.fetch_snapshot(COOKIE, now()).await.unwrap();
    assert_eq!(first.snapshot.cost_stats.unwrap().metered_cycle_usd, 2.5);
    assert_eq!(first.snapshot.account_email.as_deref(), Some("a@b.com"));
    h.clock.advance(Duration::from_secs(60));
    let second = h
        .client
        .fetch_snapshot(COOKIE, h.clock.now())
        .await
        .unwrap();
    assert_eq!(second.snapshot.cost_stats, first.snapshot.cost_stats);
    assert_eq!((h.count(EVENTS).await, h.count(SUMMARY).await), (1, 2));
    h.clock.advance(Duration::from_secs(241));
    h.client
        .fetch_snapshot(COOKIE, h.clock.now())
        .await
        .unwrap();
    assert_eq!(h.count(EVENTS).await, 2);
}

/// The incremental request starts 15 minutes before the last fetch, and the
/// cached event is kept.
#[tokio::test]
async fn test_refresh_pages_only_events_since_the_last_fetch() {
    let h = harness(now(), one_event(250)).await;
    h.client.fetch_snapshot(COOKIE, now()).await.unwrap();
    h.clock.advance(Duration::from_secs(301));
    let again = h
        .client
        .fetch_snapshot(COOKIE, h.clock.now())
        .await
        .unwrap();
    let starts = h.event_starts().await;
    assert_eq!(starts.len(), 2);
    assert_eq!(
        starts[1],
        (now() - chrono::Duration::seconds(900)).timestamp_millis()
    );
    assert_eq!(again.snapshot.cost_stats.unwrap().metered_cycle_usd, 2.5);
}

/// The overlap window replaces events instead of duplicating them.
#[tokio::test]
async fn test_overlap_events_are_replaced_not_duplicated() {
    let ten_min_ago = (now() - chrono::Duration::minutes(10)).timestamp_millis();
    let h = harness(now(), vec![(ten_min_ago, 100)]).await;
    h.client.fetch_snapshot(COOKIE, now()).await.unwrap();
    h.clock.advance(Duration::from_secs(301));
    let again = h
        .client
        .fetch_snapshot(COOKIE, h.clock.now())
        .await
        .unwrap();
    assert_eq!(again.snapshot.cost_stats.unwrap().metered_cycle_usd, 1.0);
}

/// After an hour the whole window is paged again.
#[tokio::test]
async fn test_whole_window_is_paged_again_after_the_full_refresh_interval() {
    let h = harness(now(), one_event(250)).await;
    h.client.fetch_snapshot(COOKIE, now()).await.unwrap();
    h.clock.advance(Duration::from_secs(3601));
    let again = h
        .client
        .fetch_snapshot(COOKIE, h.clock.now())
        .await
        .unwrap();
    let starts = h.event_starts().await;
    assert_eq!(starts[0], starts[1]);
    assert_eq!(again.snapshot.cost_stats.unwrap().metered_cycle_usd, 2.5);
}

/// A later events failure keeps serving the cached figures and hourly chart.
#[tokio::test]
async fn test_events_failure_reuses_cached_aggregates() {
    let h = harness(now(), one_event(400)).await;
    h.client.fetch_snapshot(COOKIE, now()).await.unwrap();
    h.fail_events.store(true, Ordering::SeqCst);
    h.clock.advance(Duration::from_secs(301));
    let again = h
        .client
        .fetch_snapshot(COOKIE, h.clock.now())
        .await
        .unwrap();
    assert_eq!(again.snapshot.cost_stats.unwrap().metered_cycle_usd, 4.0);
    assert!(!again.hourly.is_empty());
}

/// Without any cache, a failing events endpoint leaves cost stats unset.
#[tokio::test]
async fn test_events_failure_without_cache_leaves_cost_stats_nil() {
    let h = harness(now(), one_event(400)).await;
    h.fail_events.store(true, Ordering::SeqCst);
    let r = h.client.fetch_snapshot(COOKIE, now()).await.unwrap();
    assert!(r.snapshot.cost_stats.is_none());
    assert!(r.snapshot.daily_tokens.is_empty());
    assert!(r.hourly.is_empty());
    assert!(r.estimated_weight_by_day.is_empty());
}

/// A cached entry belongs to one cookie header only.
#[test]
fn test_cache_is_scoped_to_session() {
    let cache = CursorEventCache::default();
    cache.store("a", CursorCacheEntry::default());
    assert!(cache.get("a").is_some());
    cache.store("secret-cookie", CursorCacheEntry::default());
    assert!(!format!("{cache:?}").contains("secret-cookie"));
    cache.store("a", CursorCacheEntry::default());
    assert!(cache.get("b").is_none());
    cache.clear();
    assert!(cache.is_empty());
    assert!(cache.get("a").is_none());
}

/// A second account never sees the first account's cached events.
#[tokio::test]
async fn test_second_session_pages_its_own_events() {
    let h = harness(now(), one_event(250)).await;
    h.client.fetch_snapshot("a=1", now()).await.unwrap();
    h.client.fetch_snapshot("b=2", now()).await.unwrap();
    assert_eq!(h.count(EVENTS).await, 2);
}

/// Paging stops at the page cap even if the server keeps returning full pages.
#[tokio::test]
async fn test_paging_stops_at_the_page_cap() {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SUMMARY))
        .respond_with(ResponseTemplate::new(200).set_body_json(common::summary_body(now())))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/me"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let page: Vec<_> = (0..500)
        .map(|_| json!({"timestamp": now().timestamp_millis().to_string(), "model": "m"}))
        .collect();
    Mock::given(method("POST"))
        .and(path(EVENTS))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"totalUsageEventsCount": 0, "usageEventsDisplay": page})),
        )
        .mount(&server)
        .await;
    let clock = std::sync::Arc::new(tokenmon_core::ManualClock::new(now()));
    let client =
        tokenmon_provider_cursor::CursorUsageClient::new(common::cursor_http(&server.uri()), clock);
    let r = client.fetch_snapshot(COOKIE, now()).await.unwrap();
    assert!(r.snapshot.account_email.is_none());
    let posts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == EVENTS)
        .count();
    assert_eq!(posts, EVENTS_MAX_PAGES);
}

/// Stops when the reported total has been read, even on a full page.
#[tokio::test]
async fn test_paging_stops_once_the_total_is_read() {
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SUMMARY))
        .respond_with(ResponseTemplate::new(200).set_body_json(common::summary_body(now())))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/me"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let page: Vec<_> = (0..500)
        .map(|_| json!({"timestamp": now().timestamp_millis().to_string()}))
        .collect();
    Mock::given(method("POST"))
        .and(path(EVENTS))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"totalUsageEventsCount": 500, "usageEventsDisplay": page})),
        )
        .mount(&server)
        .await;
    let client = tokenmon_provider_cursor::CursorUsageClient::new(
        common::cursor_http(&server.uri()),
        std::sync::Arc::new(tokenmon_core::ManualClock::new(now())),
    );
    client.fetch_snapshot(COOKIE, now()).await.unwrap();
    let posts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == EVENTS)
        .count();
    assert_eq!(posts, 1);
}

/// A rejected summary request fails the whole fetch.
#[tokio::test]
async fn test_summary_unauthorized_fails_the_fetch() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SUMMARY))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/auth/me"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let client = tokenmon_provider_cursor::CursorUsageClient::new(
        common::cursor_http(&server.uri()),
        std::sync::Arc::new(tokenmon_core::ManualClock::new(now())),
    );
    let e = client.fetch_snapshot(COOKIE, now()).await.unwrap_err();
    assert!(is_unauthorized(&e));
}
