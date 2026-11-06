//! Ports of `OpenCodeConsoleClientTests` and the workspace tests, against a
//! fake transport (spec 03 sections 3.2 and 7.7), plus sign-in policy and
//! database path probing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokenmon_core::{BoxFuture, ProviderError, ProviderErrorContext};
use tokenmon_provider_opencode::auth::{
    auth_config, is_auth_host, is_preferred_session_cookie, is_return_page, looks_like_auth_cookie,
};
use tokenmon_provider_opencode::captions;
use tokenmon_provider_opencode::console::{
    ConsoleClient, ConsoleRequest, ConsoleResponse, ConsoleTransport, is_login_redirect_path,
    parse_go_meters, parse_orgs, parse_session_email, usd, workspace_id_from_path,
};
use tokenmon_provider_opencode::db::{DataDirEnv, candidate_db_paths, locate_db};
use tokenmon_provider_opencode::snapshot::WindowKind;

const GO_STATUS: &str = r#"{"access":{"startsAt":"2026-07-01T00:00:00.000Z","endsAt":"2026-08-01T00:00:00.000Z",
"meters":{"fiveHour":{"limitMicroCents":"1200000000","usedMicroCents":"16726299","resetsAt":"2026-07-15T18:00:00.000Z"},
"week":{"limitMicroCents":"3000000000","usedMicroCents":"359502844","resetsAt":"2026-07-20T00:00:00Z"},
"month":{"limitMicroCents":"6000000000","usedMicroCents":"359502844"}}}}"#;

fn resp(status: u16, body: &str) -> ConsoleResponse {
    ConsoleResponse {
        status,
        final_path: "/console/api/x".into(),
        cf_mitigated: false,
        is_html: false,
        body: body.as_bytes().to_vec(),
    }
}

fn seated() -> String {
    r#"{"access":{"endsAt":"2026-08-01T00:00:00Z","meters":{"month":{"limitMicroCents":"6000000000","usedMicroCents":"600000000"}}}}"#.into()
}

/// Canned responses keyed by `(path, org)`, recording every request.
#[derive(Default)]
struct Fake {
    routes: HashMap<(String, Option<String>), ConsoleResponse>,
    seen: Mutex<Vec<ConsoleRequest>>,
}

impl Fake {
    fn route(mut self, path: &str, org: Option<&str>, r: ConsoleResponse) -> Self {
        self.routes.insert((path.into(), org.map(str::to_owned)), r);
        self
    }

    fn paths(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|r| r.path.clone()).collect()
    }
}

impl ConsoleTransport for Fake {
    fn get<'a>(&'a self, request: &'a ConsoleRequest) -> BoxFuture<'a, Result<ConsoleResponse, String>> {
        self.seen.lock().unwrap().push(request.clone());
        let r = self
            .routes
            .get(&(request.path.clone(), request.org_id.clone()))
            .cloned()
            .ok_or_else(|| format!("no route {} {:?}", request.path, request.org_id));
        Box::pin(async move { r })
    }
}

struct Shared(Arc<Fake>);

impl ConsoleTransport for Shared {
    fn get<'a>(&'a self, request: &'a ConsoleRequest) -> BoxFuture<'a, Result<ConsoleResponse, String>> {
        self.0.get(request)
    }
}

const ORG_CTX: ProviderErrorContext = ProviderErrorContext::OpenCode;

fn bad(msg: &str) -> ProviderError {
    ProviderError::BadResponse(ORG_CTX, msg.into())
}

/// The fixture parses into the three meters with reset fallback data.
#[test]
fn test_parse_go_meters() {
    let m = parse_go_meters(GO_STATUS.as_bytes()).unwrap();
    let five = m.five_hour.clone().unwrap();
    assert_eq!(five.limit_usd, Some(12.0));
    assert!((five.used_usd - 0.16726299).abs() < 1e-9);
    assert!(five.resets_at.is_some());
    assert_eq!(m.week.clone().unwrap().limit_usd, Some(30.0));
    assert!((m.week.unwrap().used_usd - 3.59502844).abs() < 1e-9);
    assert_eq!(m.month.clone().unwrap().limit_usd, Some(60.0));
    assert!(m.month_resets_at.is_some());
}

/// Percentages and the monthly reset fallback.
#[test]
fn test_snapshot_percentages_and_monthly_reset_fallback() {
    let m = parse_go_meters(GO_STATUS.as_bytes()).unwrap();
    let end = m.month_resets_at;
    let snap = m.into_snapshot();
    let kinds: Vec<_> = snap.windows.iter().map(|w| w.kind).collect();
    assert_eq!(kinds, [WindowKind::Rolling5h, WindowKind::Weekly, WindowKind::Monthly]);
    let pct: Vec<f64> = snap.windows.iter().map(|w| w.used_percent()).collect();
    assert!((pct[0] - 1.393_858).abs() < 1e-4);
    assert!((pct[1] - 11.983_428).abs() < 1e-4);
    assert!((pct[2] - 5.991_714).abs() < 1e-4);
    assert_eq!(snap.windows[2].resets_at, end);
    assert!((snap.monthly_used_percent() - pct[2]).abs() < 1e-12);
    assert!(!snap.is_estimated);
}

/// No subscription: no meters; non-object body is an error.
#[test]
fn test_parse_go_meters_without_subscription_is_empty() {
    assert!(!parse_go_meters(br#"{"access":null}"#).unwrap().has_any());
    assert!(!parse_go_meters(b"{}").unwrap().has_any());
    assert_eq!(parse_go_meters(b"[1]").unwrap_err(), bad("Unrecognized Go status response."));
    assert_eq!(parse_go_meters(b"\xff\x00").unwrap_err(), bad("Unrecognized Go status response."));
}

/// Workspace listing keeps only `wrk_` ids.
#[test]
fn test_parse_orgs() {
    let ids = parse_orgs(br#"[{"id":"wrk_01KTEST123","name":"a"},{"name":"no id"},3,{"id":"org_1"}]"#).unwrap();
    assert_eq!(ids, ["wrk_01KTEST123"]);
    assert_eq!(parse_orgs(b"{}").unwrap_err(), bad("Could not read OpenCode workspaces."));
}

/// Session email.
#[test]
fn test_parse_session_email() {
    assert_eq!(parse_session_email(br#"{"user":{"email":"a@b.com"}}"#).as_deref(), Some("a@b.com"));
    assert_eq!(parse_session_email(b"{}"), None);
    assert_eq!(parse_session_email(b"junk"), None);
}

/// Micro-cents conversion, including hostile numerics.
#[test]
fn test_micro_cents_to_usd() {
    use serde_json::json;
    assert_eq!(usd(&json!("3000000000")), Some(30.0));
    assert_eq!(usd(&json!(600_000_000)), Some(6.0));
    assert_eq!(usd(&json!(null)), None);
    assert_eq!(usd(&json!("nan")), None);
    assert_eq!(usd(&json!("inf")), None);
    assert_eq!(usd(&json!("1e400")), None);
    assert_eq!(usd(&json!(true)), None);
}

/// Workspace ids from URLs.
#[test]
fn test_workspace_id_from_url() {
    assert_eq!(workspace_id_from_path("/console/wrk_ABC").as_deref(), Some("wrk_ABC"));
    assert_eq!(workspace_id_from_path("/workspace/wrk_ABC/go").as_deref(), Some("wrk_ABC"));
    assert_eq!(workspace_id_from_path("/auth"), None);
}

/// Login redirects.
#[test]
fn test_login_redirect_recognizes_console_login_pages() {
    for p in ["/console/login", "/auth/authorize", "/auth/login"] {
        assert!(is_login_redirect_path(p));
    }
    assert!(!is_login_redirect_path("/console/wrk_1"));
}

/// A seated preferred workspace is used without listing orgs.
#[tokio::test]
async fn test_preferred_workspace_with_seat_is_used_without_listing() {
    let t = Arc::new(Fake::default().route("/console/api/go/status", Some("wrk_redirect"), resp(200, &seated())));
    let c = ConsoleClient::new(Shared(Arc::clone(&t)));
    let (_, org) = c.fetch_go_usage_snapshot("c=1", Some("wrk_redirect")).await.unwrap();
    assert_eq!(org, "wrk_redirect");
    assert!(!t.paths().contains(&"/console/api/orgs".to_owned()));
}

/// Other workspaces are tried, in order, for a Go seat.
#[tokio::test]
async fn test_other_workspaces_are_tried_for_a_go_seat() {
    let t = Fake::default()
        .route("/console/api/orgs", None, resp(200, r#"[{"id":"wrk_first"},{"id":"wrk_redirect"},{"id":"wrk_go"}]"#))
        .route("/console/api/go/status", Some("wrk_first"), resp(200, "{}"))
        .route("/console/api/go/status", Some("wrk_redirect"), resp(403, "no"))
        .route("/console/api/go/status", Some("wrk_go"), resp(200, &seated()));
    let c = ConsoleClient::new(t);
    let (snap, org) = c.fetch_go_usage_snapshot("c=1", None).await.unwrap();
    assert_eq!(org, "wrk_go");
    assert!((snap.monthly_used_percent() - 10.0).abs() < 1e-9);
}

/// 403 everywhere reports the denied-workspace message (and is no auth strike).
#[tokio::test]
async fn test_forbidden_everywhere_reports_denied_workspace() {
    let t = Fake::default()
        .route("/console/api/orgs", None, resp(200, r#"[{"id":"wrk_a"},{"id":"wrk_b"}]"#))
        .route("/console/api/go/status", Some("wrk_a"), resp(403, "x"))
        .route("/console/api/go/status", Some("wrk_b"), resp(403, "x"));
    let err = ConsoleClient::new(t).fetch_go_usage_snapshot("c", None).await.unwrap_err();
    assert_eq!(err, bad("The OpenCode console denied access to this workspace."));
}

/// No workspaces and no seat produce their own messages.
#[tokio::test]
async fn test_no_workspace_and_no_seat_messages() {
    let t = Fake::default().route("/console/api/orgs", None, resp(200, "[]"));
    let err = ConsoleClient::new(t).fetch_go_usage_snapshot("c", None).await.unwrap_err();
    assert_eq!(err, bad("No OpenCode workspace on this account."));
    let t = Fake::default()
        .route("/console/api/orgs", None, resp(200, r#"[{"id":"wrk_a"}]"#))
        .route("/console/api/go/status", Some("wrk_a"), resp(200, r#"{"access":null}"#));
    let err = ConsoleClient::new(t).fetch_go_usage_snapshot("c", None).await.unwrap_err();
    assert_eq!(
        err,
        bad("No Go subscription on this account (or another member holds the Go seat).")
    );
}

/// A preferred id is tried first and skipped in the listing.
#[tokio::test]
async fn test_preferred_without_seat_falls_through_and_is_not_retried() {
    let t = Arc::new(Fake::default()
        .route("/console/api/go/status", Some("wrk_p"), resp(200, "{}"))
        .route("/console/api/orgs", None, resp(200, r#"[{"id":"wrk_p"},{"id":"wrk_q"}]"#))
        .route("/console/api/go/status", Some("wrk_q"), resp(200, &seated())));
    let c = ConsoleClient::new(Shared(Arc::clone(&t)));
    let (_, org) = c.fetch_go_usage_snapshot("c", Some("wrk_p")).await.unwrap();
    assert_eq!(org, "wrk_q");
    let p = t.paths();
    assert_eq!(p.iter().filter(|x| x.ends_with("go/status")).count(), 2);
}

/// Session expiry propagates instead of trying more workspaces.
#[tokio::test]
async fn test_unauthorized_propagates() {
    let t = Fake::default()
        .route("/console/api/orgs", None, resp(200, r#"[{"id":"wrk_a"},{"id":"wrk_b"}]"#))
        .route("/console/api/go/status", Some("wrk_a"), resp(401, ""));
    let err = ConsoleClient::new(t).fetch_go_usage_snapshot("c", None).await.unwrap_err();
    assert_eq!(err, ProviderError::Unauthorized(ORG_CTX));
}

/// A server error carries no body bytes in its message.
#[tokio::test]
async fn test_server_error_message_carries_no_body_bytes() {
    let t = Fake::default().route("/console/api/orgs", None, resp(500, "secret-token"));
    let err = ConsoleClient::new(t).list_orgs("c").await.unwrap_err();
    assert_eq!(err, bad("HTTP 500"));
    assert!(!err.to_string().contains("secret-token"));
}

/// Login redirect, bot challenge and transport failure mapping.
#[tokio::test]
async fn test_response_checks_in_order() {
    let mut login = resp(200, "{}");
    login.final_path = "/console/login".into();
    let t = Fake::default().route("/console/api/orgs", None, login);
    assert_eq!(
        ConsoleClient::new(t).list_orgs("c").await.unwrap_err(),
        ProviderError::Unauthorized(ORG_CTX)
    );
    let mut blocked = resp(403, "<html>");
    blocked.is_html = true;
    let t = Fake::default().route("/console/api/orgs", None, blocked);
    assert_eq!(
        ConsoleClient::new(t).list_orgs("c").await.unwrap_err(),
        bad("Request blocked by a bot-protection check (HTTP 403). Retrying.")
    );
    let mut cf = resp(200, "[]");
    cf.cf_mitigated = true;
    let t = Fake::default().route("/console/api/orgs", None, cf);
    assert!(ConsoleClient::new(t).list_orgs("c").await.is_err());
    let err = ConsoleClient::new(Fake::default()).list_orgs("c").await.unwrap_err();
    assert!(matches!(err, ProviderError::Network(_, _)));
}

/// Preferred workspace is kept; otherwise the first listed.
#[tokio::test]
async fn test_resolve_org_id_keeps_preferred_workspace() {
    let t = Fake::default().route("/console/api/orgs", None, resp(200, r#"[{"id":"wrk_first"}]"#));
    let c = ConsoleClient::new(t);
    assert_eq!(c.resolve_org_id("c", Some("wrk_keep")).await.unwrap(), "wrk_keep");
    assert_eq!(c.resolve_org_id("c", Some("bogus")).await.unwrap(), "wrk_first");
    assert_eq!(c.resolve_org_id("c", None).await.unwrap(), "wrk_first");
}

/// Account email lookup swallows every failure.
#[tokio::test]
async fn test_fetch_account_email_is_best_effort() {
    let t = Fake::default().route("/console/auth/session", None, resp(200, r#"{"user":{"email":"a@b.com"}}"#));
    assert_eq!(ConsoleClient::new(t).fetch_account_email("c").await.as_deref(), Some("a@b.com"));
    assert_eq!(ConsoleClient::new(Fake::default()).fetch_account_email("c").await, None);
}

/// Sign-in policy.
#[test]
fn test_auth_policy() {
    let cfg = auth_config();
    assert_eq!(cfg.store_filename_prefix, "opencode_auth_");
    assert_eq!(cfg.max_attempts, 4);
    assert!(cfg.is_essential("__Host-console_session") && cfg.is_essential("Provider"));
    assert!(!cfg.is_essential("theme"));
    assert!(is_preferred_session_cookie("AUTH") && is_preferred_session_cookie("__Host-console_session"));
    assert!(!is_preferred_session_cookie("provider"));
    assert!(looks_like_auth_cookie("x_jwt") && looks_like_auth_cookie("SID") && !looks_like_auth_cookie("theme"));
    assert!(is_auth_host("auth.opencode.ai", "/") && is_auth_host("opencode.ai", "/console/login"));
    assert!(is_return_page("opencode.ai", "/console/wrk_1"));
    assert!(!is_return_page("opencode.ai", "/console/login"));
    assert!(!is_return_page("auth.opencode.ai", "/console"));
    assert!(is_return_page("OpenCode.ai", "/x/workspace/y"));
    assert!(is_return_page("app.opencode.ai", "/auth/done"));
    assert!(!is_return_page("app.opencode.ai", "/auth/authorize"));
    assert!(!is_return_page("evil.com", "/console"));
}

/// Fallback captions.
#[test]
fn test_fallback_captions() {
    assert_eq!(captions::fallback_error(false, false), captions::NO_COOKIE);
    assert_eq!(captions::fallback_error(true, true), captions::SESSION_EXPIRED);
    assert_eq!(captions::fallback_error(true, false), captions::FETCH_FAILED);
    assert_eq!(captions::FETCH_FAILED, "Console fetch failed \u{2014} showing local estimate.");
}

fn env(xdg: Option<&str>, profile: Option<&str>, home: Option<&str>, local: Option<&str>) -> DataDirEnv {
    let p = |s: Option<&str>| s.map(PathBuf::from);
    DataDirEnv { xdg_data_home: p(xdg), user_profile: p(profile), home: p(home), local_app_data: p(local) }
}

/// Probe order, with injected environment and no real file system.
#[test]
fn test_candidate_db_paths_order() {
    let e = env(Some("/x"), Some("/u"), Some("/h"), Some("/l"));
    let got = candidate_db_paths(&e);
    let want: Vec<PathBuf> = [
        "/x/opencode/opencode.db",
        "/u/.local/share/opencode/opencode.db",
        "/h/.local/share/opencode/opencode.db",
        "/l/opencode/opencode.db",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    assert_eq!(got, want);
    assert!(candidate_db_paths(&DataDirEnv::default()).is_empty());
    // Same profile and home deduplicate.
    assert_eq!(candidate_db_paths(&env(None, Some("/u"), Some("/u"), None)).len(), 1);
}

/// The first existing candidate wins; otherwise the first candidate is reported.
#[test]
fn test_locate_db() {
    let e = env(Some("/x"), Some("/u"), None, Some("/l"));
    let only = |p: &'static str| move |c: &Path| c == Path::new(p);
    assert_eq!(
        locate_db(&e, only("/l/opencode/opencode.db")),
        Some(PathBuf::from("/l/opencode/opencode.db"))
    );
    assert_eq!(
        locate_db(&e, |_| false),
        Some(PathBuf::from("/x/opencode/opencode.db"))
    );
    assert_eq!(locate_db(&DataDirEnv::default(), |_| true), None);
}
