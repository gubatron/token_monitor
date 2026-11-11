//! Overview panel (`OverviewPanelView`, spec 04 section 3.5 and spec 01
//! sections 4.4-4.5): provider cards and the hourly "Usage Today" chart.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike, Utc};
use tokenmon_core::usage::UsageSource;
use tokenmon_core::{ProviderId, Srgb};

use super::common::{PanelContext, geometry};
use tokenmon_core::overview::format::format_tokens_at;

use super::format;
use super::opencode_openrouter::{ORANGE, PURPLE};

/// What a card's sign-in button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAction {
    /// Open the provider's sign-in window.
    OpenSignIn(ProviderId),
    /// Select the provider's tab (OpenRouter's API-key flow).
    SelectTab(ProviderId),
}

/// Raw state for one Overview card.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewCardInput {
    /// Provider.
    pub provider: ProviderId,
    /// A snapshot exists.
    pub has_snapshot: bool,
    /// Headline percent (`None` renders 0).
    pub percent: Option<f64>,
    /// Reset instant for the caption.
    pub reset_at: Option<DateTime<Utc>>,
    /// `auth.isSignedIn`.
    pub signed_in: bool,
    /// `auth.needsSignIn` (Grokbot: Cursor's).
    pub needs_sign_in: bool,
    /// OpenRouter: `keyUsageUSD`.
    pub key_usage_usd: Option<f64>,
    /// OpenRouter: `budgetSource != nil`.
    pub has_budget_source: bool,
    /// OpenRouter: `remainingUSD`.
    pub remaining_usd: Option<f64>,
}

/// A rendered Overview provider card.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewCardVm {
    /// Provider.
    pub provider: ProviderId,
    /// Title (display name).
    pub title: String,
    /// Meta label (`— Weekly`), uppercased by the renderer.
    pub meta: &'static str,
    /// Trailing text (`N% Used` or a USD amount).
    pub used_text: String,
    /// Fill fraction `0..=1`.
    pub fraction: f64,
    /// Fill color.
    pub color: Srgb,
    /// Caption line.
    pub caption: Option<String>,
    /// Caption uses the tertiary color.
    pub caption_is_tertiary: bool,
    /// Sign-in button label and action.
    pub sign_in: Option<(String, CardAction)>,
    /// Sign-out control target.
    pub sign_out: Option<ProviderId>,
}

fn meta(p: ProviderId) -> &'static str {
    match p {
        ProviderId::Grok | ProviderId::Claude | ProviderId::Grokbot => "— Weekly",
        ProviderId::OpenCode | ProviderId::Cursor => "— Monthly",
        ProviderId::ChatGpt => "— Codex",
        ProviderId::OpenRouter => "— Credits",
        ProviderId::Overview => "",
    }
}

/// Builds one card following the table in spec 04 section 3.5.
pub fn build_card(c: &OverviewCardInput, ctx: &PanelContext) -> OverviewCardVm {
    let p = c.provider;
    let percent = c.percent.unwrap_or(0.0);
    let (sign_in, sign_out) = match p {
        ProviderId::Grok => (
            (!c.signed_in || c.needs_sign_in).then(|| {
                (
                    format!("Sign In to {}…", p.display_name()),
                    CardAction::OpenSignIn(p),
                )
            }),
            (c.signed_in && !c.needs_sign_in).then_some(p),
        ),
        ProviderId::Grokbot => (
            (c.needs_sign_in && !c.has_snapshot).then(|| {
                (
                    "Sign In to Cursor…".to_owned(),
                    CardAction::OpenSignIn(ProviderId::Cursor),
                )
            }),
            None,
        ),
        ProviderId::OpenRouter => (
            (c.needs_sign_in && !c.has_snapshot).then(|| {
                (
                    format!("Sign In to {}…", p.display_name()),
                    CardAction::SelectTab(p),
                )
            }),
            (!c.needs_sign_in).then_some(p),
        ),
        _ => (
            (c.needs_sign_in && !c.has_snapshot).then(|| {
                (
                    format!("Sign In to {}…", p.display_name()),
                    CardAction::OpenSignIn(p),
                )
            }),
            (!c.needs_sign_in).then_some(p),
        ),
    };
    let no_data = format!("No {} data yet", p.display_name());
    let (caption, tertiary) = if p == ProviderId::OpenRouter {
        let cap = if c.has_snapshot && !c.has_budget_source {
            Some(format!(
                "No credit limit — {} spent on this key",
                format::usd(c.key_usage_usd.unwrap_or(0.0))
            ))
        } else if let Some(r) = c.remaining_usd {
            Some(format!("{} of credits left", format::usd(r)))
        } else if !c.has_snapshot {
            Some(no_data)
        } else {
            None
        };
        (cap, true)
    } else if let Some(t) = c.reset_at {
        (Some(ctx.reset_caption(t)), false)
    } else if !c.has_snapshot {
        (Some(no_data), true)
    } else {
        (None, false)
    };
    let used_text = if p == ProviderId::OpenRouter && c.has_snapshot && c.percent.is_none() {
        format::usd(c.key_usage_usd.unwrap_or(0.0))
    } else {
        format::percent_used(percent)
    };
    OverviewCardVm {
        provider: p,
        title: p.display_name().to_owned(),
        meta: meta(p),
        used_text,
        fraction: format::clamp_percent(percent) / 100.0,
        color: if p == ProviderId::OpenCode {
            PURPLE
        } else {
            p.panel_accent()
        },
        caption,
        caption_is_tertiary: tertiary,
        sign_in,
        sign_out,
    }
}

// ------------------------------ Hourly chart --------------------------------

/// A stacked series of the Overview hourly chart, in fixed bottom-to-top order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HourSeries {
    /// Grok.
    Grok,
    /// Grokbot.
    Grokbot,
    /// Cursor.
    Cursor,
    /// Claude.
    Claude,
    /// OpenCode Go.
    OpenCodeGo,
    /// OpenCode Zen.
    OpenCodeZen,
}

impl HourSeries {
    /// Stack order, bottom to top.
    pub const STACK_ORDER: [Self; 6] = [
        Self::Grok,
        Self::Grokbot,
        Self::Cursor,
        Self::Claude,
        Self::OpenCodeGo,
        Self::OpenCodeZen,
    ];

    /// Legend/tooltip title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Grokbot => "Grokbot",
            Self::Cursor => "Cursor",
            Self::Claude => "Claude",
            Self::OpenCodeGo => "Go",
            Self::OpenCodeZen => "Zen",
        }
    }

    /// Series color (Go orange 0.85, Zen purple 0.78, others accent 0.85).
    pub fn color(self) -> Srgb {
        match self {
            Self::Grok => ProviderId::Grok.panel_accent(),
            Self::Grokbot => ProviderId::Grokbot.panel_accent(),
            Self::Cursor => ProviderId::Cursor.panel_accent(),
            Self::Claude => ProviderId::Claude.panel_accent(),
            Self::OpenCodeGo => ORANGE.with_alpha(0.85),
            Self::OpenCodeZen => PURPLE.with_alpha(0.78),
        }
    }

    /// Tooltip quota wording.
    pub fn quota_label(self) -> &'static str {
        match self {
            Self::Grok | Self::Grokbot => "of weekly",
            Self::Claude => "of 5-hour window",
            Self::Cursor | Self::OpenCodeGo | Self::OpenCodeZen => "of weekly share",
        }
    }

    /// Where this series' tokens were counted: OpenCode's from `opencode.db`
    /// and Claude's from the transcripts, both on this PC; Grok's, Grokbot's
    /// and Cursor's are the account's.
    pub fn token_source(self) -> UsageSource {
        match self {
            Self::Grok | Self::Grokbot | Self::Cursor => UsageSource::Account,
            Self::Claude | Self::OpenCodeGo | Self::OpenCodeZen => UsageSource::ThisComputer,
        }
    }

    /// Grokbot never shows tokens.
    pub fn shows_tokens(self) -> bool {
        self != Self::Grokbot
    }
}

/// One series' activity in one hour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HourActivityInput {
    /// Series.
    pub series: HourSeries,
    /// Quota percent consumed in the hour.
    pub quota_percent: f64,
    /// Tokens, where known.
    pub tokens: Option<u64>,
}

/// One slice of the hour's tooltip: `Cursor 1.20% of weekly share · 3.4M`, or
/// `Go 1.20% of weekly share · 3.4M tokens on this PC` when the tokens were
/// counted on this computer.
pub fn segment_help(a: &HourActivityInput) -> String {
    let quota = format!(
        "{} {:.2}% {}",
        a.series.title(),
        a.quota_percent,
        a.series.quota_label()
    );
    match a.tokens.filter(|t| *t > 0 && a.series.shows_tokens()) {
        Some(t) => {
            let count = i64::try_from(t).unwrap_or(i64::MAX);
            format!(
                "{quota} · {}",
                format_tokens_at(count, a.series.token_source())
            )
        }
        None => quota,
    }
}

/// Rendered stacked segment.
#[derive(Debug, Clone, PartialEq)]
pub struct HourSegmentVm {
    /// Height in px.
    pub height: f64,
    /// Color.
    pub color: Srgb,
}

/// Rendered hour column.
#[derive(Debug, Clone, PartialEq)]
pub struct HourColumnVm {
    /// Hour `0..24`.
    pub hour: u32,
    /// Bar height in px (0 = idle capsule).
    pub bar_height: f64,
    /// Segments bottom to top.
    pub segments: Vec<HourSegmentVm>,
    /// Hover tooltip.
    pub tooltip: String,
    /// Axis label when shown.
    pub axis_label: Option<String>,
}

/// Legend entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendVm {
    /// Title.
    pub title: &'static str,
    /// Color.
    pub color: Srgb,
}

/// Rendered hourly chart.
#[derive(Debug, Clone, PartialEq)]
pub struct HourlyChartVm {
    /// Columns, or empty when there is no activity.
    pub columns: Vec<HourColumnVm>,
    /// Legend of series with activity.
    pub legend: Vec<LegendVm>,
    /// Message when empty.
    pub empty_message: Option<&'static str>,
}

/// Bar height: 0 if no activity, else `min(68, max(3, 68*a/max))`.
///
/// ```
/// use tokenmon_ui::panels::overview::bar_height;
/// assert_eq!(bar_height(5.0, 10.0, 60.0), 30.0);
/// assert_eq!(bar_height(0.001, 10.0, 60.0), 3.0);
/// assert_eq!(bar_height(0.0, 10.0, 60.0), 0.0);
/// ```
pub fn bar_height(activity: f64, max_activity: f64, track: f64) -> f64 {
    if activity <= 0.0 || max_activity <= 0.0 {
        0.0
    } else {
        (track * activity / max_activity).max(3.0).min(track)
    }
}

/// Proportional segment heights with a lifted minimum for thin slices.
///
/// ```
/// use tokenmon_ui::panels::overview::segment_heights;
/// assert_eq!(segment_heights(&[1.0, 3.0], 40.0, 1.0), vec![10.0, 30.0]);
/// ```
pub fn segment_heights(activities: &[f64], total: f64, minimum: f64) -> Vec<f64> {
    let sum: f64 = activities.iter().map(|a| a.max(0.0)).sum();
    if sum <= 0.0 {
        return vec![0.0; activities.len()];
    }
    let mut heights: Vec<f64> = activities
        .iter()
        .map(|a| total * a.max(0.0) / sum)
        .collect();
    let nonzero = heights.iter().filter(|h| **h > 0.0).count();
    if nonzero >= 2 && minimum * nonzero as f64 <= total {
        let mut lift = 0.0;
        for h in heights.iter_mut().filter(|h| **h > 0.0 && **h < minimum) {
            lift += minimum - *h;
            *h = minimum;
        }
        if let Some((i, _)) = heights.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) {
            heights[i] -= lift;
        }
    }
    heights
}

/// 6..=22 widened to include the earliest and latest hour with activity.
pub fn visible_hour_range(active_hours: &[u32]) -> (u32, u32) {
    let lo = active_hours.iter().copied().min().map_or(6, |h| h.min(6));
    let hi = active_hours.iter().copied().max().map_or(22, |h| h.max(22));
    (lo, hi)
}

/// Hours that get an axis label: both ends plus multiples of 3 at least 2 from both ends.
pub fn axis_hours(range: (u32, u32)) -> Vec<u32> {
    (range.0..=range.1)
        .filter(|h| {
            *h == range.0
                || *h == range.1
                || (h.is_multiple_of(3) && h - range.0 >= 2 && range.1 - h >= 2)
        })
        .collect()
}

/// Axis label text: 0/12 -> `12`, 13..23 -> hour-12, else hour.
pub fn axis_label(hour: u32) -> String {
    match hour {
        0 | 12 => "12".to_owned(),
        13..=23 => (hour - 12).to_string(),
        h => h.to_string(),
    }
}

/// Tooltip hour label: `12a`, `1a`..`11a`, `12p`, `1p`..`11p`.
pub fn hour_label(hour: u32) -> String {
    let h12 = if hour.is_multiple_of(12) {
        12
    } else {
        hour % 12
    };
    format!("{h12}{}", if hour < 12 { "a" } else { "p" })
}

/// Builds the hourly chart from 24 hours of per-series activity.
pub fn build_hourly_chart(hours: &[Vec<HourActivityInput>], is_today: bool) -> HourlyChartVm {
    let total = |h: &[HourActivityInput]| h.iter().map(|a| a.quota_percent.max(0.0)).sum::<f64>();
    let active: Vec<u32> = hours
        .iter()
        .enumerate()
        .filter(|(_, h)| total(h) > 0.0)
        .map(|(i, _)| i as u32)
        .collect();
    let range = visible_hour_range(&active);
    let visible = || (range.0..=range.1).filter(|h| (*h as usize) < hours.len());
    let max = visible()
        .map(|h| total(&hours[h as usize]))
        .fold(0.0, f64::max);
    if max <= 0.0 {
        return HourlyChartVm {
            columns: Vec::new(),
            legend: Vec::new(),
            empty_message: Some(if is_today {
                "No Grok, Grokbot, OpenCode, Cursor, or Claude activity yet today."
            } else {
                "No activity recorded on this day."
            }),
        };
    }
    let axis = axis_hours(range);
    let columns = visible()
        .map(|hour| {
            let acts = &hours[hour as usize];
            let ordered: Vec<&HourActivityInput> = HourSeries::STACK_ORDER
                .iter()
                .filter_map(|s| acts.iter().find(|a| a.series == *s))
                .filter(|a| a.quota_percent > 0.0)
                .collect();
            let bar = bar_height(total(acts), max, geometry::STEM_HEIGHT);
            let heights = segment_heights(
                &ordered.iter().map(|a| a.quota_percent).collect::<Vec<_>>(),
                bar,
                1.0,
            );
            let tooltip = ordered
                .iter()
                .map(|a| segment_help(a))
                .collect::<Vec<_>>()
                .join(" · ");
            HourColumnVm {
                hour,
                bar_height: bar,
                segments: ordered
                    .iter()
                    .zip(heights)
                    .map(|(a, height)| HourSegmentVm {
                        height,
                        color: a.series.color(),
                    })
                    .collect(),
                tooltip: format!("{}: {tooltip}", hour_label(hour)),
                axis_label: axis.contains(&hour).then(|| axis_label(hour)),
            }
        })
        .collect();
    let legend = HourSeries::STACK_ORDER
        .iter()
        .filter(|s| {
            visible().any(|h| {
                hours[h as usize]
                    .iter()
                    .any(|a| a.series == **s && a.quota_percent > 0.0)
            })
        })
        .map(|s| LegendVm {
            title: s.title(),
            color: s.color(),
        })
        .collect();
    HourlyChartVm {
        columns,
        legend,
        empty_message: None,
    }
}

/// `Usage Today` / `Usage Yesterday` / `Usage EEE, MMM d`.
pub fn day_title(day_offset: i32, date: NaiveDate) -> String {
    match day_offset {
        0 => "Usage Today".to_owned(),
        -1 => "Usage Yesterday".to_owned(),
        _ => format!("Usage {}", date.format("%a, %b %-d")),
    }
}

/// `canGoBack`: an earlier saved day exists.
pub fn can_go_back(day: NaiveDate, earliest: Option<NaiveDate>) -> bool {
    earliest.is_some_and(|e| e < day)
}

/// Today's local midnight plus the next 7, as UTC instants.
pub fn midnight_schedule(now: DateTime<Utc>, tz: FixedOffset) -> Vec<DateTime<Utc>> {
    let local = now.with_timezone(&tz);
    let start = local
        .with_hour(0)
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .map_or(now, |d| d.with_timezone(&Utc));
    (0..8).map(|k| start + Duration::days(k)).collect()
}

/// Daily usage card input.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewDailyInput {
    /// 0 today, -1 yesterday, ...
    pub day_offset: i32,
    /// 24 hours of activity.
    pub hours: Vec<Vec<HourActivityInput>>,
    /// Earliest saved history day.
    pub earliest_day: Option<NaiveDate>,
}

/// Rendered daily usage card.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewDailyVm {
    /// Title.
    pub title: String,
    /// Navigation arrows shown.
    pub has_navigation: bool,
    /// Back arrow enabled.
    pub can_go_back: bool,
    /// Forward arrow enabled.
    pub can_go_forward: bool,
    /// Chart.
    pub chart: HourlyChartVm,
}

/// Overview panel input.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewInput {
    /// App version without `v` (`None` -> `1.0.0`).
    pub version: Option<String>,
    /// Cards in user order.
    pub cards: Vec<OverviewCardInput>,
    /// Daily usage card.
    pub daily: OverviewDailyInput,
}

/// Rendered Overview panel.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewPanelVm {
    /// `TokenMon`.
    pub title: &'static str,
    /// `v<version>`.
    pub version_text: String,
    /// Cards.
    pub cards: Vec<OverviewCardVm>,
    /// Daily usage card.
    pub daily: OverviewDailyVm,
}

/// Builds the Overview panel.
pub fn build_overview(input: &OverviewInput, ctx: &PanelContext) -> OverviewPanelVm {
    let d = &input.daily;
    let date = ctx.today() + Duration::days(i64::from(d.day_offset));
    let back = can_go_back(date, d.earliest_day);
    OverviewPanelVm {
        title: "TokenMon",
        version_text: format!("v{}", input.version.as_deref().unwrap_or("1.0.0")),
        cards: input.cards.iter().map(|c| build_card(c, ctx)).collect(),
        daily: OverviewDailyVm {
            title: day_title(d.day_offset, date),
            has_navigation: back || d.day_offset < 0,
            can_go_back: back,
            can_go_forward: d.day_offset < 0,
            chart: build_hourly_chart(&d.hours, d.day_offset == 0),
        },
    }
}
