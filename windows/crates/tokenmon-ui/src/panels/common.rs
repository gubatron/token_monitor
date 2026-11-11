//! Types and builders shared by every panel view model: context, status,
//! header, tracks, footers, metric grids, model rows and the daily-budget bars.
//!
//! Inputs are small structs owned by this crate; `From` impls from the
//! provider crates' snapshot types are added once those land.

use chrono::{DateTime, NaiveDate, Utc};
use tokenmon_core::usage::{Calendar, DailyBudget, PaceHeadroom, api_value_text};
use tokenmon_core::{ProviderId, Srgb};

use crate::kit::week::WeekOffset;
use crate::kit::{daily as kit_daily, hover as kit_hover};

use super::format;

/// Fixed geometry from spec 04 section 3.
pub mod geometry {
    /// Panel width.
    pub const PANEL_WIDTH: f64 = 420.0;
    /// Daily bar stem height.
    pub const STEM_HEIGHT: f64 = 68.0;
    /// Daily bar stem width.
    pub const STEM_WIDTH: f64 = 12.0;
    /// Inner width available to a card's content (420 - 2*12 - 2*12).
    pub const CARD_INNER_WIDTH: f64 = 372.0;
}

/// Wall-clock inputs for display: "now" and the local calendar.
///
/// The calendar resolves the UTC offset per instant, so a reset caption on
/// the far side of a DST change uses the offset in effect at the reset, as
/// Swift's `TimeZone.current` does.
#[derive(Debug, Clone, Copy)]
pub struct PanelContext<'a> {
    /// Current instant (from the injected clock).
    pub now: DateTime<Utc>,
    /// Local calendar used for all calendar formatting.
    pub calendar: Calendar<'a>,
}

impl PanelContext<'_> {
    /// Local calendar day containing `now`.
    pub fn today(&self) -> NaiveDate {
        self.calendar.date_of(self.now)
    }

    /// `Resets EEE dd MMMM h:mma` caption.
    pub fn reset_caption(&self, at: DateTime<Utc>) -> String {
        format::reset_caption(at, self.calendar.offset_at(at))
    }
}

/// Auth and poller state a panel needs (merged from `AuthSession` + poller).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderStatus {
    /// `auth.needsSignIn`.
    pub needs_sign_in: bool,
    /// `auth.lastAuthError != nil`.
    pub has_auth_error: bool,
    /// `poller.isRefreshing`.
    pub is_refreshing: bool,
    /// `poller.lastError`.
    pub last_error: Option<String>,
    /// `poller.lastRefreshedAt`.
    pub last_refreshed_at: Option<DateTime<Utc>>,
}

/// Which of the four top-level states a panel is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    /// No session; shows the sign-in prompt.
    SignedOut,
    /// Session rejected; shows the error and `Sign In Again…` (Grok).
    SessionExpired,
    /// Signed in but no snapshot yet (loading or error text).
    Waiting,
    /// A snapshot exists; the usage body is present.
    Usage,
}

/// A button that opens a provider's sign-in window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInAction {
    /// Button label.
    pub label: String,
    /// Provider whose sign-in window opens (Grokbot opens Cursor).
    pub provider: ProviderId,
}

impl SignInAction {
    /// `Sign In to <displayName>…`.
    pub fn to_provider(provider: ProviderId) -> Self {
        Self {
            label: format!("Sign In to {}…", provider.display_name()),
            provider,
        }
    }

    /// `Sign In Again…`.
    pub fn again(provider: ProviderId) -> Self {
        Self {
            label: "Sign In Again…".to_owned(),
            provider,
        }
    }
}

/// Provider header row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderVm {
    /// Provider whose logo is shown.
    pub provider: ProviderId,
    /// Title text.
    pub title: String,
    /// Optional trailing meta label (plan name, budget source).
    pub meta: Option<String>,
}

impl HeaderVm {
    /// Header with the given title and no meta.
    pub fn new(provider: ProviderId, title: &str) -> Self {
        Self {
            provider,
            title: title.to_owned(),
            meta: None,
        }
    }

    /// Header with a meta label.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<String>) -> Self {
        self.meta = meta;
        self
    }
}

/// A whole provider panel: frame plus an optional provider-specific body.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelVm<U> {
    /// Header row.
    pub header: HeaderVm,
    /// Top-level state.
    pub kind: PanelKind,
    /// Explanatory text for non-usage states.
    pub message: Option<String>,
    /// Primary sign-in button for non-usage states.
    pub sign_in: Option<SignInAction>,
    /// Usage body when `kind == Usage`.
    pub usage: Option<U>,
}

impl<U> PanelVm<U> {
    /// A non-usage panel.
    pub fn frame(
        header: HeaderVm,
        kind: PanelKind,
        message: Option<String>,
        sign_in: Option<SignInAction>,
    ) -> Self {
        Self {
            header,
            kind,
            message,
            sign_in,
            usage: None,
        }
    }

    /// A usage panel.
    pub fn with_usage(header: HeaderVm, usage: U) -> Self {
        Self {
            header,
            kind: PanelKind::Usage,
            message: None,
            sign_in: None,
            usage: Some(usage),
        }
    }
}

/// `Refreshing…` while refreshing, else the error, else the fallback.
pub fn waiting_message(status: &ProviderStatus, fallback: &str) -> String {
    if status.is_refreshing {
        "Refreshing…".to_owned()
    } else {
        status
            .last_error
            .clone()
            .unwrap_or_else(|| fallback.to_owned())
    }
}

/// Builds the standard (Claude/ChatGPT/OpenCode/Cursor/Grokbot) non-usage
/// frames. Returns `None` when a snapshot exists (caller builds the body).
pub fn standard_frame<U>(
    header: HeaderVm,
    status: &ProviderStatus,
    has_snapshot: bool,
    sign_in_text: &str,
    sign_in: SignInAction,
) -> Result<PanelVm<U>, HeaderVm> {
    if status.needs_sign_in && !has_snapshot {
        return Ok(PanelVm::frame(
            header,
            PanelKind::SignedOut,
            Some(sign_in_text.to_owned()),
            Some(sign_in),
        ));
    }
    if !has_snapshot {
        let action = status.needs_sign_in.then_some(sign_in);
        return Ok(PanelVm::frame(
            header,
            PanelKind::Waiting,
            Some(waiting_message(status, "No usage data yet.")),
            action,
        ));
    }
    Err(header)
}

/// A labelled usage track (`SlimUsageTrack`).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackVm {
    /// Left label.
    pub label: String,
    /// Whether the label row is drawn.
    pub show_label: bool,
    /// `N% Used`.
    pub used_text: String,
    /// Fill fraction `0..=1`.
    pub fraction: f64,
    /// Fill color.
    pub color: Srgb,
    /// Caption under the track.
    pub caption: Option<String>,
    /// Caption is drawn in the warning (orange) color.
    pub caption_is_warning: bool,
}

impl TrackVm {
    /// Builds a track from a raw percent.
    pub fn new(label: &str, percent: f64, color: Srgb, caption: Option<String>) -> Self {
        Self {
            label: label.to_owned(),
            show_label: true,
            used_text: format::percent_used(percent),
            fraction: format::clamp_percent(percent) / 100.0,
            color,
            caption,
            caption_is_warning: false,
        }
    }

    /// Hides the label row.
    #[must_use]
    pub fn without_label(mut self) -> Self {
        self.show_label = false;
        self
    }
}

/// Stale-data caption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleVm {
    /// The error message.
    pub message: String,
    /// `Showing data from <age> ago.` when a refresh time is known.
    pub age_line: Option<String>,
}

impl StaleVm {
    /// Builds from the poller state; `None` when there is no error.
    pub fn from_status(status: &ProviderStatus, ctx: &PanelContext) -> Option<Self> {
        status.last_error.as_ref().map(|message| Self {
            message: message.clone(),
            age_line: status
                .last_refreshed_at
                .map(|t| format!("Showing data from {} ago.", format::age_text(t, ctx.now))),
        })
    }
}

/// Sign-out control (idle icon, confirm text on click).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignOutVm {
    /// Provider signed out (Grokbot names Cursor).
    pub provider: ProviderId,
    /// `Sign out of <name>?`.
    pub confirm_text: String,
}

impl SignOutVm {
    /// Tooltip of the idle icon.
    pub const TOOLTIP: &'static str = "Sign out";
    /// Tooltip of the confirm button.
    pub const CONFIRM_TOOLTIP: &'static str = "Confirm sign out";
    /// Tooltip of the cancel button.
    pub const CANCEL_TOOLTIP: &'static str = "Cancel";

    /// Control for `provider`.
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            confirm_text: format!("Sign out of {}?", provider.display_name()),
        }
    }
}

/// Bottom-of-panel controls shared by all usage bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct FooterVm {
    /// Stale caption when `lastError` is set.
    pub stale: Option<StaleVm>,
    /// `Sign In Again…` when the session needs sign-in.
    pub sign_in_again: Option<SignInAction>,
    /// Sign-out control.
    pub sign_out: Option<SignOutVm>,
}

impl FooterVm {
    /// Standard footer: stale caption, optional sign-in-again, sign-out of `sign_out_provider`.
    pub fn standard(
        status: &ProviderStatus,
        ctx: &PanelContext,
        sign_in_provider: ProviderId,
        sign_out_provider: ProviderId,
    ) -> Self {
        Self {
            stale: StaleVm::from_status(status, ctx),
            sign_in_again: status
                .needs_sign_in
                .then(|| SignInAction::again(sign_in_provider)),
            sign_out: Some(SignOutVm::new(sign_out_provider)),
        }
    }
}

/// One cell of a metric grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricVm {
    /// Header (uppercased by the renderer).
    pub title: String,
    /// Value text.
    pub value: String,
}

impl MetricVm {
    /// Builds a cell.
    pub fn new(title: &str, value: String) -> Self {
        Self {
            title: title.to_owned(),
            value,
        }
    }
}

/// One model row with a share bar.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRowVm {
    /// Display name.
    pub name: String,
    /// Right-aligned cost text.
    pub cost_text: String,
    /// Optional small tag (`revealed`).
    pub tag: Option<String>,
    /// Share bar fraction `0..=1`.
    pub share_fraction: f64,
    /// Share bar color.
    pub color: Srgb,
    /// Floating label shown while the row is hovered (Claude's models).
    pub hover: Option<HoverCardVm>,
}

/// A floating hover label's text: the daily bars and the Claude model rows
/// both fill it, so every label looks the same (Swift `HoverLabelCard`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HoverCardVm {
    /// First line naming what it describes.
    pub title: String,
    /// Secondary one-line summary under the title.
    pub caption: Option<String>,
    /// Large token total; `None` hides the token block.
    pub total: Option<String>,
    /// Value beside the total (`≈ $12.40`).
    pub value: Option<String>,
    /// Breakdown rows under the total.
    pub rows: Vec<(String, String)>,
    /// A second grid under a rule (the day's top models).
    pub extra_rows: Vec<(String, String)>,
    /// Caption under a rule (last month's totals).
    pub note: Option<String>,
    /// Semibold closing line (the change against last month).
    pub verdict: Option<String>,
}

/// A models card: collapsible or not, with an optional `View all` toggle.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelsCardVm {
    /// Section title.
    pub title: String,
    /// Header is a collapsible button.
    pub collapsible: bool,
    /// Section is expanded (always true when not collapsible).
    pub expanded: bool,
    /// Rows shown now (empty while collapsed).
    pub rows: Vec<ModelRowVm>,
    /// `View all` / `Show less` when there are more than 3 rows.
    pub toggle_label: Option<&'static str>,
}

impl ModelsCardVm {
    /// Rows shown before `View all`.
    pub const COLLAPSED_ROWS: usize = 3;

    /// Applies the 3-row truncation.
    pub fn new(
        title: &str,
        collapsible: bool,
        expanded: bool,
        mut rows: Vec<ModelRowVm>,
        show_all: bool,
    ) -> Self {
        let expanded = expanded || !collapsible;
        if !expanded {
            rows.clear();
        }
        let toggle_label = (rows.len() > Self::COLLAPSED_ROWS).then_some(if show_all {
            "Show less"
        } else {
            "View all"
        });
        if !show_all {
            rows.truncate(Self::COLLAPSED_ROWS);
        }
        Self {
            title: title.to_owned(),
            collapsible,
            expanded,
            rows,
            toggle_label,
        }
    }
}

// ---------------------------------------------------------------------------
// Daily budget bars
// ---------------------------------------------------------------------------

/// Per-day token breakdown for the hover label.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokensInput {
    /// Total tokens.
    pub total: u64,
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Cache-read tokens.
    pub cache_read: u64,
    /// Cache-write tokens.
    pub cache_write: u64,
    /// Estimated cost.
    pub cost_usd: f64,
    /// Some usage had no known price.
    pub has_unpriced: bool,
    /// `(model name, tokens)` sorted by the caller.
    pub models: Vec<(String, u64)>,
}

/// One bar of a daily-budget chart.
#[derive(Debug, Clone, PartialEq)]
pub struct DayInput {
    /// Local calendar day.
    pub date: NaiveDate,
    /// Percent of the pool spent that day.
    pub spent: f64,
    /// Daily allowance percent.
    pub budget: f64,
    /// Day belongs to the window before an early provider reset.
    pub is_prior_window: bool,
    /// Spent exceeded the allowance.
    pub is_over_budget: bool,
    /// Optional per-day tokens.
    pub tokens: Option<TokensInput>,
}

/// `weekly` or `monthly` (hover wording).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowanceNoun {
    /// Weekly pool.
    Weekly,
    /// Monthly pool.
    Monthly,
}

impl AllowanceNoun {
    /// Lowercase noun.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

/// Which footer rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterKind {
    /// Generic `DailyBudgetBarsView` rules.
    Generic,
    /// Grok chart: pace, else the estimate note when `show_estimate_note`.
    GrokWeek {
        /// `week.isEstimated || !week.hasDailyData`.
        show_estimate_note: bool,
    },
}

/// Everything a daily-budget card needs.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyBudgetInput {
    /// Bars, oldest first.
    pub days: Vec<DayInput>,
    /// Fill color (accent).
    pub accent: Srgb,
    /// Weekly or monthly.
    pub noun: AllowanceNoun,
    /// Pace headroom from `DailyBudget::pace_headroom`; its caption comes from
    /// core's `DailyBudget::pace_caption`.
    pub pace: Option<PaceHeadroom>,
    /// Footer rules.
    pub footer_kind: FooterKind,
    /// Live footer shown (false for past weeks).
    pub show_live_footer: bool,
    /// Week arrows shown.
    pub has_navigation: bool,
    /// Next arrow enabled (`offset < 0`).
    pub can_go_next: bool,
    /// Minimum fill fraction that draws a fill (0.005 generic, 0 for Grok).
    pub fill_threshold: f64,
}

/// Hover label content for a day.
#[derive(Debug, Clone, PartialEq)]
pub struct HoverVm {
    /// `EEEE, MMM d`.
    pub title: String,
    /// Usage line (not for future days).
    pub usage_line: Option<String>,
    /// Token block when tokens exist.
    pub tokens: Option<HoverTokensVm>,
}

/// Token block of the hover label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverTokensVm {
    /// `Format.tokens(total)`.
    pub total_text: String,
    /// `≈ $X.XX[+]` when cost > 0.
    pub cost_text: Option<String>,
    /// Input/Output/Cache read/Cache write rows.
    pub rows: Vec<(String, String)>,
    /// Up to 3 model rows.
    pub models: Vec<(String, String)>,
}

/// One rendered column.
#[derive(Debug, Clone, PartialEq)]
pub struct DayBarVm {
    /// Fill height in px (0 = no fill drawn).
    pub fill_height: f64,
    /// Fill opacity (0.45 prior window, else 1).
    pub fill_opacity: f64,
    /// `N%` label.
    pub value_label: String,
    /// Two-character weekday.
    pub weekday: String,
    /// Label opacity (0.5 for future days).
    pub label_opacity: f64,
    /// Accent fill color.
    pub color: Srgb,
    /// Hover label.
    pub hover: HoverVm,
}

/// A rendered daily-budget card.
#[derive(Debug, Clone, PartialEq)]
pub struct DailyBudgetVm {
    /// Section title.
    pub title: String,
    /// `%.1f%%` of the daily allowance, when budget > 0.
    pub allowance_text: Option<String>,
    /// Range label (`October 13 – 19`).
    pub range_label: String,
    /// Week arrows shown.
    pub has_navigation: bool,
    /// Previous arrow enabled (always when shown).
    pub can_go_previous: bool,
    /// Next arrow enabled.
    pub can_go_next: bool,
    /// Bars.
    pub bars: Vec<DayBarVm>,
    /// Footer caption.
    pub footer: Option<String>,
}

/// Week range label: `October 13 – 19` or `September 29 – Oct 5` (delegates to the kit).
pub fn range_label(first: NaiveDate, last: NaiveDate) -> String {
    kit_daily::range_label(first, last)
}

/// Hover usage line for a non-future day (delegates to the kit).
pub fn usage_line(day: &DayInput, noun: AllowanceNoun) -> String {
    kit_hover::usage_line(
        kit_hover::DayUsage {
            spent: day.spent,
            budget: day.budget,
            is_prior_window: day.is_prior_window,
        },
        noun.as_str(),
    )
}

fn hover_tokens(t: &TokensInput) -> Option<HoverTokensVm> {
    (t.total > 0).then(|| HoverTokensVm {
        total_text: format::tokens(t.total),
        cost_text: api_value_text(Some(t.cost_usd), t.has_unpriced),
        rows: vec![
            ("Input".to_owned(), format::tokens(t.input)),
            ("Output".to_owned(), format::tokens(t.output)),
            ("Cache read".to_owned(), format::tokens(t.cache_read)),
            ("Cache write".to_owned(), format::tokens(t.cache_write)),
        ],
        models: t
            .models
            .iter()
            .take(3)
            .map(|(n, c)| (n.clone(), format::tokens(*c)))
            .collect(),
    })
}

fn build_bar(day: &DayInput, input: &DailyBudgetInput, today: NaiveDate) -> DayBarVm {
    let future = day.date > today;
    let fraction = if day.budget > 0.0 {
        day.spent / day.budget
    } else {
        0.0
    };
    // Grok draws nothing for a day at or below 0.05% of the weekly pool
    // (`GrokDailyUsageBuilder.fillFraction`); other providers use the
    // input's fraction threshold.
    let above_threshold = match input.footer_kind {
        FooterKind::GrokWeek { .. } => day.spent > GROK_MIN_DAY_PERCENT,
        FooterKind::Generic => fraction > input.fill_threshold,
    };
    let drawn = above_threshold && !future;
    let h = geometry::STEM_HEIGHT;
    DayBarVm {
        fill_height: if drawn {
            (h * fraction).max(6.0).min(h)
        } else {
            0.0
        },
        fill_opacity: if day.is_prior_window { 0.45 } else { 1.0 },
        value_label: format::percent_label(day.spent),
        weekday: day.date.format("%a").to_string().chars().take(2).collect(),
        label_opacity: if future { 0.5 } else { 1.0 },
        color: input.accent,
        hover: HoverVm {
            title: day.date.format("%A, %b %-d").to_string(),
            usage_line: (!future).then(|| usage_line(day, input.noun)),
            tokens: day.tokens.as_ref().and_then(hover_tokens),
        },
    }
}

fn footer(input: &DailyBudgetInput, today: NaiveDate) -> Option<String> {
    if !input.show_live_footer {
        return None;
    }
    if let Some(p) = input.pace.as_ref().and_then(DailyBudget::pace_caption) {
        return Some(p);
    }
    match input.footer_kind {
        FooterKind::GrokWeek { show_estimate_note } => {
            show_estimate_note.then(|| "Bars update between polls · week totals above".to_owned())
        }
        FooterKind::Generic => {
            let budget = input.days.first().map_or(0.0, |d| d.budget);
            if input.days.iter().all(|d| d.spent <= 0.001) {
                Some(format!("No usage yet · {budget:.1}%/day"))
            } else if input
                .days
                .iter()
                .any(|d| d.date == today && d.is_over_budget)
            {
                Some("Over today's allowance".to_owned())
            } else {
                None
            }
        }
    }
}

/// Smallest weekly-pool percent a Grok day must exceed to draw a bar.
const GROK_MIN_DAY_PERCENT: f64 = 0.05;

/// Grok's week range, abbreviated on both ends: `Nov 3 – Nov 9`
/// (`GrokModels.swift` `rangeLabel`).
///
/// ```
/// use chrono::NaiveDate;
/// use tokenmon_ui::panels::common::grok_range_label;
/// let d = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
/// assert_eq!(grok_range_label(d(11, 3), d(11, 9)), "Nov 3 \u{2013} Nov 9");
/// ```
pub fn grok_range_label(first: NaiveDate, last: NaiveDate) -> String {
    format!(
        "{} \u{2013} {}",
        first.format("%b %-d"),
        last.format("%b %-d")
    )
}

/// Builds the daily-budget card; `None` when there are no days (card hidden).
pub fn build_daily_budget(input: &DailyBudgetInput, ctx: &PanelContext) -> Option<DailyBudgetVm> {
    let (first, last) = (input.days.first()?, input.days.last()?);
    let today = ctx.today();
    let budget = first.budget;
    Some(DailyBudgetVm {
        title: "Daily Budget".to_owned(),
        allowance_text: (budget > 0.0).then(|| format!("{budget:.1}%")),
        range_label: match input.footer_kind {
            FooterKind::GrokWeek { .. } => grok_range_label(first.date, last.date),
            FooterKind::Generic => range_label(first.date, last.date),
        },
        has_navigation: input.has_navigation,
        can_go_previous: input.has_navigation,
        can_go_next: input.can_go_next,
        bars: input
            .days
            .iter()
            .map(|d| build_bar(d, input, today))
            .collect(),
        footer: footer(input, today),
    })
}

/// `WeekOffset.previous`: unbounded (delegates to the kit).
pub fn week_previous(offset: i32) -> i32 {
    WeekOffset(offset).previous().0
}

/// `WeekOffset.next`: never past the current week (delegates to the kit).
pub fn week_next(offset: i32) -> i32 {
    WeekOffset(offset).next().0
}

/// `WeekOffset.canGoNext` (delegates to the kit).
pub fn week_can_go_next(offset: i32) -> bool {
    WeekOffset(offset).can_go_next()
}
