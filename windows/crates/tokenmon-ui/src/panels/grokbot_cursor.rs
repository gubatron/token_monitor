//! Grokbot (`GrokbotPanelView`, spec 02 section 2.6) and Cursor
//! (`CursorPanelView`, spec 02 section 3.5) panels.

use chrono::{DateTime, Utc};
use tokenmon_core::ProviderId;

use super::common::{
    DailyBudgetInput, DailyBudgetVm, FooterVm, HeaderVm, MetricVm, PanelContext, PanelVm,
    ProviderStatus, SignInAction, TrackVm, build_daily_budget, standard_frame,
};
use super::format;

// ------------------------------- Grokbot -----------------------------------

/// Grokbot snapshot input.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokbotSnapshotInput {
    /// Pool name (`Weekly`, `Mixed`, ...).
    pub pool_name: String,
    /// Used percent.
    pub used_percent: f64,
    /// `entitlement.captionText`.
    pub entitlement_caption: String,
    /// Plan has an included allowance.
    pub has_included_allowance: bool,
    /// Reset instant.
    pub resets_at: Option<DateTime<Utc>>,
    /// Daily bars (hidden when empty or `None`).
    pub daily: Option<DailyBudgetInput>,
}

/// Grokbot panel input. `status` carries **Cursor's** `needs_sign_in`.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokbotInput {
    /// Auth and poller state.
    pub status: ProviderStatus,
    /// Snapshot.
    pub snapshot: Option<GrokbotSnapshotInput>,
}

/// Grokbot usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokbotUsage {
    /// `Usage Pool <pool>`.
    pub pool_title: String,
    /// `N% Used`.
    pub pool_used_text: String,
    /// Entitlement caption.
    pub entitlement_caption: String,
    /// Allowance track (label hidden) when included.
    pub track: Option<TrackVm>,
    /// Text when the plan has no included allowance.
    pub no_allowance_text: Option<&'static str>,
    /// Daily bars.
    pub daily: Option<DailyBudgetVm>,
    /// Footer (sign-out names Cursor).
    pub footer: FooterVm,
}

/// Builds the Grokbot panel.
pub fn build_grokbot(input: &GrokbotInput, ctx: &PanelContext) -> PanelVm<GrokbotUsage> {
    let header = HeaderVm::new(ProviderId::Grokbot, "Grokbot");
    let s = &input.status;
    let action = SignInAction {
        label: "Sign In to Grokbot…".to_owned(),
        provider: ProviderId::Cursor,
    };
    let header = match standard_frame(
        header,
        s,
        input.snapshot.is_some(),
        "Sign in to Cursor to load your weekly Grokbot allowance. Grokbot is billed through Cursor even when a SuperGrok plan pays for it.",
        action,
    ) {
        Ok(frame) => return frame,
        Err(h) => h,
    };
    let Some(snap) = &input.snapshot else {
        unreachable!("standard_frame handles the no-snapshot case")
    };
    let track = snap.has_included_allowance.then(|| {
        TrackVm::new(
            "",
            snap.used_percent,
            ProviderId::Grokbot.panel_accent(),
            snap.resets_at.map(|t| ctx.reset_caption(t)),
        )
        .without_label()
    });
    PanelVm::with_usage(
        header,
        GrokbotUsage {
            pool_title: format!("Usage Pool {}", snap.pool_name),
            pool_used_text: format::percent_used(snap.used_percent),
            entitlement_caption: snap.entitlement_caption.clone(),
            track,
            no_allowance_text: (!snap.has_included_allowance)
                .then_some("This plan has no included Bot allowance — usage bills on demand."),
            daily: snap
                .has_included_allowance
                .then(|| snap.daily.as_ref().and_then(|d| build_daily_budget(d, ctx)))
                .flatten(),
            footer: FooterVm::standard(s, ctx, ProviderId::Cursor, ProviderId::Cursor),
        },
    )
}

// -------------------------------- Cursor -----------------------------------

/// One Cursor usage pool.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorPoolInput {
    /// `trackLabel` (Total, Auto, API).
    pub label: String,
    /// Used percent.
    pub used_percent: f64,
    /// Pool reset (falls back to the snapshot's).
    pub resets_at: Option<DateTime<Utc>>,
}

/// Cursor cycle cost stats.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorCostInput {
    /// Metered spend this cycle.
    pub metered_cycle_usd: f64,
    /// Total tokens.
    pub cycle_tokens: u64,
    /// Input tokens.
    pub cycle_input_tokens: u64,
    /// Output tokens.
    pub cycle_output_tokens: u64,
}

/// Cursor snapshot input.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorSnapshotInput {
    /// `displayPlanName`.
    pub plan_name: String,
    /// Pools in order (Total, Auto, API).
    pub pools: Vec<CursorPoolInput>,
    /// Snapshot-level reset.
    pub resets_at: Option<DateTime<Utc>>,
    /// Monthly daily bars.
    pub daily: Option<DailyBudgetInput>,
    /// Cost stats.
    pub cost: Option<CursorCostInput>,
}

/// Cursor panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorInput {
    /// Auth and poller state.
    pub status: ProviderStatus,
    /// Snapshot.
    pub snapshot: Option<CursorSnapshotInput>,
}

/// Cursor usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorUsage {
    /// `Usage Pool Monthly`.
    pub pool_title: String,
    /// One track per pool.
    pub tracks: Vec<TrackVm>,
    /// Daily bars.
    pub daily: Option<DailyBudgetVm>,
    /// 2x2 grid.
    pub metrics: Option<Vec<MetricVm>>,
    /// Footer.
    pub footer: FooterVm,
}

/// Builds the Cursor panel.
pub fn build_cursor(input: &CursorInput, ctx: &PanelContext) -> PanelVm<CursorUsage> {
    let header = HeaderVm::new(ProviderId::Cursor, "Cursor");
    let s = &input.status;
    let header = match standard_frame(
        header,
        s,
        input.snapshot.is_some(),
        "Sign in to cursor.com to load Total, Auto, and API usage.",
        SignInAction::to_provider(ProviderId::Cursor),
    ) {
        Ok(frame) => return frame,
        Err(h) => h,
    };
    let Some(snap) = &input.snapshot else {
        unreachable!("standard_frame handles the no-snapshot case")
    };
    let accent = ProviderId::Cursor.panel_accent();
    let tracks = snap
        .pools
        .iter()
        .map(|p| {
            TrackVm::new(
                &p.label,
                p.used_percent,
                accent,
                p.resets_at.or(snap.resets_at).map(|t| ctx.reset_caption(t)),
            )
        })
        .collect();
    let metrics = snap.cost.as_ref().map(|c| {
        vec![
            MetricVm::new("Monthly spend", format::usd(c.metered_cycle_usd)),
            MetricVm::new("Total tokens", format::tokens(c.cycle_tokens)),
            MetricVm::new("Input tokens", format::tokens(c.cycle_input_tokens)),
            MetricVm::new("Output tokens", format::tokens(c.cycle_output_tokens)),
        ]
    });
    PanelVm::with_usage(
        header.with_meta(Some(snap.plan_name.clone())),
        CursorUsage {
            pool_title: "Usage Pool Monthly".to_owned(),
            tracks,
            daily: snap.daily.as_ref().and_then(|d| build_daily_budget(d, ctx)),
            metrics,
            footer: FooterVm::standard(s, ctx, ProviderId::Cursor, ProviderId::Cursor),
        },
    )
}
