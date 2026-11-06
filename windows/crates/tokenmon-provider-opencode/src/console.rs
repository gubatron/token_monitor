//! OpenCode console client (spec 03 section 3.2).
//!
//! The HTTP layer is injected through [`ConsoleTransport`] so the workspace
//! order, 403 handling and response mapping are tested without a network.

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokenmon_core::{BoxFuture, ProviderError, ProviderErrorContext};

use crate::snapshot::{OpenCodeSnapshot, WindowKind, WindowUsage};

const CTX: ProviderErrorContext = ProviderErrorContext::OpenCode;

/// One console GET, relative to `https://opencode.ai`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleRequest {
    /// Path and query, e.g. `/console/api/go/status`.
    pub path: String,
    /// `Cookie` header value.
    pub cookie: String,
    /// `x-org-id` header, when org-scoped.
    pub org_id: Option<String>,
}

/// What the transport saw, after redirects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleResponse {
    /// HTTP status.
    pub status: u16,
    /// Path of the final URL after redirects.
    pub final_path: String,
    /// A `cf-mitigated` header was present.
    pub cf_mitigated: bool,
    /// The body looks like HTML (content type or leading `<`).
    pub is_html: bool,
    /// Raw body.
    pub body: Vec<u8>,
}

/// Performs the GET (20 s timeout, `Accept: application/json`, TokenMon user
/// agent). Errors are transport failures as text.
pub trait ConsoleTransport: Send + Sync {
    /// Executes `request`.
    fn get<'a>(
        &'a self,
        request: &'a ConsoleRequest,
    ) -> BoxFuture<'a, Result<ConsoleResponse, String>>;
}

/// One Go meter.
#[derive(Debug, Clone, PartialEq)]
pub struct GoMeter {
    /// Limit in USD, if given.
    pub limit_usd: Option<f64>,
    /// Used USD (0 when absent).
    pub used_usd: f64,
    /// Reset instant, if given.
    pub resets_at: Option<DateTime<Utc>>,
}

/// Parsed `go/status` body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GoMeters {
    /// Five-hour meter.
    pub five_hour: Option<GoMeter>,
    /// Weekly meter.
    pub week: Option<GoMeter>,
    /// Monthly meter.
    pub month: Option<GoMeter>,
    /// End of the subscription period, the monthly reset fallback.
    pub month_resets_at: Option<DateTime<Utc>>,
}

impl GoMeters {
    /// Whether any meter is present (the account holds a Go seat).
    pub fn has_any(&self) -> bool {
        self.five_hour.is_some() || self.week.is_some() || self.month.is_some()
    }

    /// Builds the official snapshot (`snapshot(meters, now)`).
    pub fn into_snapshot(self) -> OpenCodeSnapshot {
        let make = |kind: WindowKind, m: Option<GoMeter>, fallback: Option<DateTime<Utc>>| {
            m.map(|m| WindowUsage {
                kind,
                used_usd: m.used_usd,
                limit_usd: m.limit_usd.unwrap_or_else(|| kind.default_limit_usd()),
                resets_at: m.resets_at.or(fallback),
                session_count: 0,
            })
        };
        let windows = [
            make(WindowKind::Rolling5h, self.five_hour, None),
            make(WindowKind::Weekly, self.week, None),
            make(WindowKind::Monthly, self.month, self.month_resets_at),
        ]
        .into_iter()
        .flatten()
        .collect();
        OpenCodeSnapshot {
            windows,
            ..OpenCodeSnapshot::default()
        }
    }
}

/// Micro-cents (1e8 per USD) from a JSON string or number; `None` for
/// anything else and for non-finite values such as `"nan"` or `"1e400"`.
pub fn usd(value: &Value) -> Option<f64> {
    let raw = match value {
        Value::String(s) => s.parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    let usd = raw / 100_000_000.0;
    usd.is_finite().then_some(usd)
}

fn iso(value: Option<&Value>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?.as_str()?)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn parse_meter(value: Option<&Value>) -> Option<GoMeter> {
    let obj = value?.as_object()?;
    Some(GoMeter {
        limit_usd: obj.get("limitMicroCents").and_then(usd),
        used_usd: obj.get("usedMicroCents").and_then(usd).unwrap_or(0.0),
        resets_at: iso(obj.get("resetsAt")),
    })
}

/// Parses `go/status`.
///
/// # Errors
/// `BadResponse("Unrecognized Go status response.")` when the body is not a
/// JSON object.
pub fn parse_go_meters(data: &[u8]) -> Result<GoMeters, ProviderError> {
    let root: Value = serde_json::from_slice(data).unwrap_or(Value::Null);
    let Some(root) = root.as_object() else {
        return Err(ProviderError::BadResponse(
            CTX,
            "Unrecognized Go status response.".into(),
        ));
    };
    let Some(access) = root.get("access").and_then(Value::as_object) else {
        return Ok(GoMeters::default());
    };
    let Some(meters) = access.get("meters").and_then(Value::as_object) else {
        return Ok(GoMeters::default());
    };
    Ok(GoMeters {
        five_hour: parse_meter(meters.get("fiveHour")),
        week: parse_meter(meters.get("week")),
        month: parse_meter(meters.get("month")),
        month_resets_at: iso(access.get("endsAt")),
    })
}

/// Parses `/console/api/orgs` into the ordered `wrk_` ids.
///
/// # Errors
/// `BadResponse("Could not read OpenCode workspaces.")` unless the body is a
/// JSON array.
pub fn parse_orgs(data: &[u8]) -> Result<Vec<String>, ProviderError> {
    let Ok(Value::Array(rows)) = serde_json::from_slice::<Value>(data) else {
        return Err(ProviderError::BadResponse(
            CTX,
            "Could not read OpenCode workspaces.".into(),
        ));
    };
    Ok(rows
        .iter()
        .filter_map(|r| r.as_object()?.get("id")?.as_str())
        .filter(|id| id.starts_with("wrk_"))
        .map(str::to_owned)
        .collect())
}

/// `root.user.email`, best effort.
pub fn parse_session_email(data: &[u8]) -> Option<String> {
    let root: Value = serde_json::from_slice(data).ok()?;
    Some(root.get("user")?.get("email")?.as_str()?.to_owned())
}

/// First path segment starting `wrk_`.
pub fn workspace_id_from_path(path: &str) -> Option<String> {
    path.split('/')
        .find(|s| s.starts_with("wrk_"))
        .map(str::to_owned)
}

/// Whether a final path is one of the login pages.
pub fn is_login_redirect_path(path: &str) -> bool {
    ["/console/login", "/auth/authorize", "/auth/login"]
        .iter()
        .any(|p| path.contains(p))
}

/// Internal outcome of a single GET.
#[derive(Debug)]
enum GetError {
    OrgForbidden,
    Provider(ProviderError),
}

impl From<ProviderError> for GetError {
    fn from(e: ProviderError) -> Self {
        Self::Provider(e)
    }
}

/// Maps a response to a body or an error, in the spec's order.
fn check(resp: ConsoleResponse, org_id: Option<&str>) -> Result<Vec<u8>, GetError> {
    let s = resp.status;
    if resp.cf_mitigated || (s == 403 && resp.is_html) {
        return Err(ProviderError::BadResponse(
            CTX,
            format!("Request blocked by a bot-protection check (HTTP {s}). Retrying."),
        )
        .into());
    }
    if is_login_redirect_path(&resp.final_path) {
        return Err(ProviderError::Unauthorized(CTX).into());
    }
    if s == 403 && org_id.is_some() {
        return Err(GetError::OrgForbidden);
    }
    if s == 401 || s == 403 {
        return Err(ProviderError::Unauthorized(CTX).into());
    }
    if s >= 400 {
        return Err(ProviderError::BadResponse(CTX, format!("HTTP {s}")).into());
    }
    Ok(resp.body)
}

/// Console client over an injected transport.
#[derive(Debug)]
pub struct ConsoleClient<T> {
    transport: T,
}

/// Outcome of trying one workspace.
enum Attempt {
    Seated(GoMeters),
    NoSeat,
    Forbidden,
}

impl<T: ConsoleTransport> ConsoleClient<T> {
    /// Wraps a transport.
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    async fn get(
        &self,
        path: &str,
        cookie: &str,
        org_id: Option<&str>,
    ) -> Result<Vec<u8>, GetError> {
        let request = ConsoleRequest {
            path: path.to_owned(),
            cookie: cookie.to_owned(),
            org_id: org_id.map(str::to_owned),
        };
        let resp = self
            .transport
            .get(&request)
            .await
            .map_err(|e| ProviderError::Network(CTX, e))?;
        check(resp, org_id)
    }

    fn lower(e: GetError) -> ProviderError {
        match e {
            GetError::Provider(p) => p,
            GetError::OrgForbidden => ProviderError::Unauthorized(CTX),
        }
    }

    /// Workspace ids of the account, in order.
    ///
    /// # Errors
    /// Transport, auth and parse failures.
    pub async fn list_orgs(&self, cookie: &str) -> Result<Vec<String>, ProviderError> {
        let body = self
            .get("/console/api/orgs", cookie, None)
            .await
            .map_err(Self::lower)?;
        parse_orgs(&body)
    }

    /// Account email; every failure is swallowed.
    pub async fn fetch_account_email(&self, cookie: &str) -> Option<String> {
        let body = self.get("/console/auth/session", cookie, None).await.ok()?;
        parse_session_email(&body)
    }

    async fn attempt(&self, cookie: &str, org: &str) -> Result<Attempt, ProviderError> {
        match self.get("/console/api/go/status", cookie, Some(org)).await {
            Ok(body) => {
                let meters = parse_go_meters(&body)?;
                Ok(if meters.has_any() {
                    Attempt::Seated(meters)
                } else {
                    Attempt::NoSeat
                })
            }
            Err(GetError::OrgForbidden) => Ok(Attempt::Forbidden),
            Err(GetError::Provider(p)) => Err(p),
        }
    }

    /// Official Go usage. Tries `known_org` first, then every listed
    /// workspace in order; 403 on a workspace means try the next one.
    ///
    /// # Errors
    /// Session expiry and transport errors propagate; otherwise the three
    /// spec messages for no workspace, all forbidden and no seat.
    pub async fn fetch_go_usage_snapshot(
        &self,
        cookie: &str,
        known_org: Option<&str>,
    ) -> Result<(OpenCodeSnapshot, String), ProviderError> {
        let preferred = known_org.filter(|o| o.starts_with("wrk_"));
        let (mut tried, mut forbidden) = (0_u32, 0_u32);
        let mut outcome = |a: Attempt, org: &str| -> Option<(OpenCodeSnapshot, String)> {
            tried += 1;
            match a {
                Attempt::Seated(m) => return Some((m.into_snapshot(), org.to_owned())),
                Attempt::Forbidden => forbidden += 1,
                Attempt::NoSeat => {}
            }
            None
        };
        if let Some(org) = preferred {
            let a = self.attempt(cookie, org).await?;
            if let Some(found) = outcome(a, org) {
                return Ok(found);
            }
        }
        for org in self.list_orgs(cookie).await? {
            if Some(org.as_str()) == preferred {
                continue;
            }
            let a = self.attempt(cookie, &org).await?;
            if let Some(found) = outcome(a, &org) {
                return Ok(found);
            }
        }
        let msg = if tried == 0 {
            "No OpenCode workspace on this account."
        } else if forbidden == tried {
            "The OpenCode console denied access to this workspace."
        } else {
            "No Go subscription on this account (or another member holds the Go seat)."
        };
        Err(ProviderError::BadResponse(CTX, msg.into()))
    }

    /// Preferred `wrk_` id if given, else the first listed workspace.
    ///
    /// # Errors
    /// `BadResponse("No OpenCode workspace on this account.")` when none.
    pub async fn resolve_org_id(
        &self,
        cookie: &str,
        preferred: Option<&str>,
    ) -> Result<String, ProviderError> {
        if let Some(p) = preferred.filter(|p| p.starts_with("wrk_")) {
            return Ok(p.to_owned());
        }
        self.list_orgs(cookie)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                ProviderError::BadResponse(CTX, "No OpenCode workspace on this account.".into())
            })
    }
}
