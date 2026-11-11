//! Claude (`ClaudePanelView`, spec 03 section 1.7) and ChatGPT
//! (`ChatGPTPanelView`, spec 03 section 2.7) panels.

use chrono::{DateTime, Utc};
use tokenmon_core::ProviderId;
use tokenmon_core::usage::UsageSource;
use tokenmon_provider_claude::ClaudeCodeUsageSummary;
use tokenmon_provider_claude::hover::ClaudeModelHoverDetail;

use super::common::{
    DailyBudgetInput, DailyBudgetVm, FooterVm, HeaderVm, HoverCardVm, HoverTotalVm, MetricGridVm,
    MetricVm, ModelRowVm, ModelsCardVm, PanelContext, PanelVm, ProviderStatus, SignInAction,
    TrackVm, build_daily_budget, standard_frame,
};
use super::format;

/// A quota window (`fiveHour`, `sevenDay`, ChatGPT primary/secondary).
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInput {
    /// Provider-supplied label, if any.
    pub label: Option<String>,
    /// Used percent (`None` renders 0).
    pub used_percent: Option<f64>,
    /// Reset instant.
    pub resets_at: Option<DateTime<Utc>>,
}

// -------------------------------- Claude -----------------------------------

/// A Claude Code model row.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeModelInput {
    /// `ClaudeModelName.display`.
    pub name: String,
    /// No price known.
    pub is_unpriced: bool,
    /// Cost in USD.
    pub cost_usd: f64,
    /// Share of month spend, percent.
    pub share_percent: f64,
    /// What the row's hover label says; `None` shows no label.
    pub hover: Option<ClaudeModelHoverDetail>,
}

/// Token+spend totals for one period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodeTotalsInput {
    /// Tokens.
    pub tokens: u64,
    /// Spend.
    pub cost_usd: f64,
    /// Some usage unpriced.
    pub has_unpriced: bool,
}

/// Claude Code usage summary (local transcripts).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCodeInput {
    /// This calendar month.
    pub month: CodeTotalsInput,
    /// All time.
    pub all_time: CodeTotalsInput,
    /// Models this month.
    pub month_models: Vec<CodeModelInput>,
    /// Persisted `claudeModels` expanded state.
    pub models_expanded: bool,
}

/// Claude snapshot input.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeSnapshotInput {
    /// `Usage Pool <Hourly|Weekly|Mixed>` name.
    pub pool_name: String,
    /// 5-hour window.
    pub five_hour: Option<WindowInput>,
    /// 7-day window.
    pub seven_day: Option<WindowInput>,
    /// `Weekly` or `Weekly · <scope>`.
    pub weekly_label: String,
    /// `poller.weeklyResetsAt()` (live or projected).
    pub weekly_resets_at: Option<DateTime<Utc>>,
    /// Daily bars.
    pub daily: Option<DailyBudgetInput>,
}

/// Claude panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeInput {
    /// Auth and poller state.
    pub status: ProviderStatus,
    /// Snapshot.
    pub snapshot: Option<ClaudeSnapshotInput>,
    /// Claude Code section source (present when a projects dir exists).
    pub code: Option<ClaudeCodeInput>,
}

/// Claude Code section.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeCodeVm {
    /// 2x2 grid.
    pub metrics: MetricGridVm,
    /// `Models This Month` card.
    pub models: Option<ModelsCardVm>,
}

/// Claude usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeUsage {
    /// `Usage Pool X`.
    pub pool_title: String,
    /// `5-Hour Window` and weekly tracks.
    pub tracks: Vec<TrackVm>,
    /// Daily bars.
    pub daily: Option<DailyBudgetVm>,
    /// Footer.
    pub footer: FooterVm,
}

/// Whole Claude panel: frame plus the Claude Code section.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudePanelVm {
    /// Main frame.
    pub panel: PanelVm<ClaudeUsage>,
    /// Claude Code section (in every state when present).
    pub code: Option<ClaudeCodeVm>,
}

/// Claude Code's figures are read from this PC's transcripts, not from the
/// account, so the stats card and "Models This Month" are tagged "This PC".
pub const CLAUDE_CODE_SOURCE: UsageSource = ClaudeCodeUsageSummary::SOURCE;

fn spend(t: &CodeTotalsInput) -> String {
    format!(
        "{}{}",
        format::usd(t.cost_usd),
        if t.has_unpriced { "+" } else { "" }
    )
}

/// The floating label of a model row (Swift `ClaudeModelHoverLabel`): title,
/// token total with its API-rate value, the breakdown, then last month's
/// totals and the change against it.
pub fn model_hover(d: &ClaudeModelHoverDetail) -> HoverCardVm {
    HoverCardVm {
        title: d.title.clone(),
        caption: None,
        total: Some(HoverTotalVm {
            text: format::tokens(u64::try_from(d.tokens).unwrap_or(0)),
            source: CLAUDE_CODE_SOURCE,
        }),
        value: Some(d.value_text.clone()),
        rows: d
            .breakdown
            .iter()
            .map(|r| (r.label.clone(), r.value.clone()))
            .collect(),
        extra_rows: Vec::new(),
        note: d.previous_line.clone(),
        verdict: Some(d.change_line.clone()),
    }
}

/// Builds the Claude Code section.
pub fn build_code_section(c: &ClaudeCodeInput) -> ClaudeCodeVm {
    let accent = ProviderId::Claude.panel_accent();
    let rows: Vec<ModelRowVm> = c
        .month_models
        .iter()
        .map(|m| ModelRowVm {
            name: m.name.clone(),
            cost_text: if m.is_unpriced {
                "No price".to_owned()
            } else {
                format::usd(m.cost_usd)
            },
            tag: None,
            share_fraction: format::clamp_percent(m.share_percent) / 100.0,
            color: accent,
            hover: m.hover.as_ref().map(model_hover),
        })
        .collect();
    ClaudeCodeVm {
        metrics: MetricGridVm::new(
            CLAUDE_CODE_SOURCE,
            vec![
                MetricVm::new("Tokens this month", format::tokens(c.month.tokens)),
                MetricVm::new("Spend this month", spend(&c.month)),
                MetricVm::new("Total tokens", format::tokens(c.all_time.tokens)),
                MetricVm::new("Total spend", spend(&c.all_time)),
            ],
        ),
        // The Mac lists every model under a collapsible header, with no
        // "View all" / "Show less" toggle.
        models: (!rows.is_empty()).then(|| ModelsCardVm {
            toggle_label: None,
            ..ModelsCardVm::new(
                "Models This Month",
                CLAUDE_CODE_SOURCE,
                true,
                c.models_expanded,
                rows,
                true,
            )
        }),
    }
}

/// Builds the Claude panel.
pub fn build_claude(input: &ClaudeInput, ctx: &PanelContext) -> ClaudePanelVm {
    let code = input.code.as_ref().map(build_code_section);
    let header = HeaderVm::new(ProviderId::Claude, "Claude");
    let s = &input.status;
    let header = match standard_frame(
        header,
        s,
        input.snapshot.is_some(),
        "Sign in to claude.ai to load your 5-hour and weekly usage.",
        SignInAction::to_provider(ProviderId::Claude),
    ) {
        Ok(panel) => return ClaudePanelVm { panel, code },
        Err(h) => h,
    };
    let Some(snap) = &input.snapshot else {
        unreachable!("standard_frame handles the no-snapshot case")
    };
    let accent = ProviderId::Claude.panel_accent();
    let mut tracks = Vec::with_capacity(2);
    let five = snap.five_hour.as_ref();
    tracks.push(TrackVm::new(
        "5-Hour Window",
        five.and_then(|w| w.used_percent).unwrap_or(0.0),
        accent,
        five.and_then(|w| w.resets_at).map(|t| ctx.reset_caption(t)),
    ));
    tracks.push(TrackVm::new(
        &snap.weekly_label,
        snap.seven_day
            .as_ref()
            .and_then(|w| w.used_percent)
            .unwrap_or(0.0),
        accent,
        snap.weekly_resets_at.map(|t| ctx.reset_caption(t)),
    ));
    ClaudePanelVm {
        panel: PanelVm::with_usage(
            header,
            ClaudeUsage {
                pool_title: format!("Usage Pool {}", snap.pool_name),
                tracks,
                daily: snap.daily.as_ref().and_then(|d| build_daily_budget(d, ctx)),
                footer: FooterVm::standard(s, ctx, ProviderId::Claude, ProviderId::Claude),
            },
        ),
        code,
    }
}

// -------------------------------- ChatGPT ----------------------------------

/// ChatGPT snapshot input.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatGptSnapshotInput {
    /// `displayPlanName`.
    pub plan_name: String,
    /// `Usage Pool X`.
    pub pool_name: String,
    /// Primary (5-hour) window.
    pub primary: Option<WindowInput>,
    /// Secondary (weekly) window.
    pub secondary: Option<WindowInput>,
    /// Limit reached flag.
    pub limit_reached: bool,
    /// Codex access allowed.
    pub allowed: bool,
}

/// ChatGPT panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatGptInput {
    /// Auth and poller state.
    pub status: ProviderStatus,
    /// Snapshot.
    pub snapshot: Option<ChatGptSnapshotInput>,
}

/// ChatGPT usage body.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatGptUsage {
    /// `Usage Pool X`.
    pub pool_title: String,
    /// Tracks.
    pub tracks: Vec<TrackVm>,
    /// Blocked-access note.
    pub blocked_text: Option<&'static str>,
    /// Footer.
    pub footer: FooterVm,
}

/// Builds the ChatGPT panel.
pub fn build_chatgpt(input: &ChatGptInput, ctx: &PanelContext) -> PanelVm<ChatGptUsage> {
    let header = HeaderVm::new(ProviderId::ChatGpt, "ChatGPT");
    let s = &input.status;
    let header = match standard_frame(
        header,
        s,
        input.snapshot.is_some(),
        "Sign in to chatgpt.com to load your 5-hour and weekly Codex usage.",
        SignInAction::to_provider(ProviderId::ChatGpt),
    ) {
        Ok(frame) => return frame,
        Err(h) => h,
    };
    let Some(snap) = &input.snapshot else {
        unreachable!("standard_frame handles the no-snapshot case")
    };
    let accent = ProviderId::ChatGpt.panel_accent();
    let mut tracks = Vec::with_capacity(2);
    let (plabel, pused) = match &snap.primary {
        Some(w) => (
            w.label
                .clone()
                .unwrap_or_else(|| "5-Hour Window".to_owned()),
            w.used_percent.unwrap_or(0.0),
        ),
        None => ("5-Hour Window".to_owned(), 0.0),
    };
    let pcap = if snap.limit_reached {
        Some("Limit reached".to_owned())
    } else {
        snap.primary
            .as_ref()
            .and_then(|w| w.resets_at)
            .map(|t| ctx.reset_caption(t))
    };
    tracks.push(TrackVm::new(&plabel, pused, accent, pcap));
    // The Weekly track is always drawn, "0% Used" when the window is missing.
    let secondary = snap.secondary.as_ref();
    tracks.push(TrackVm::new(
        secondary
            .and_then(|w| w.label.as_deref())
            .unwrap_or("Weekly"),
        secondary.and_then(|w| w.used_percent).unwrap_or(0.0),
        accent,
        secondary
            .and_then(|w| w.resets_at)
            .map(|t| ctx.reset_caption(t)),
    ));
    PanelVm::with_usage(
        header.with_meta(Some(snap.plan_name.clone())),
        ChatGptUsage {
            pool_title: format!("Usage Pool {}", snap.pool_name),
            tracks,
            blocked_text: (!snap.allowed)
                .then_some("Codex access is currently blocked for this account."),
            footer: FooterVm::standard(s, ctx, ProviderId::ChatGpt, ProviderId::ChatGpt),
        },
    )
}
