//! Chart geometry, labels, legend, tooltips and day navigation (spec 01
//! section 4.5), as plain data for the Slint panel to render.

use std::ops::RangeInclusive;

use chrono::{DateTime, Days, NaiveDate, NaiveTime, TimeZone, Utc};

use super::format::{format_tokens, hour_label};
use super::series::OverviewSeries;
use super::usage::{HourUsage, ProviderDayHourlyUsage};
use crate::provider::{ProviderId, Srgb};

/// Height of the bar track in points.
pub const TRACK_HEIGHT: f64 = 68.0;
/// Width of a bar stem in points.
pub const STEM_WIDTH: f64 = 12.0;
/// Corner radius of a bar in points.
pub const CORNER_RADIUS: f64 = 5.0;
/// Height of the axis label row; the chart frame is `TRACK_HEIGHT + AXIS_HEIGHT`.
pub const AXIS_HEIGHT: f64 = 19.0;
/// Height of the capsule drawn for an hour without activity.
pub const EMPTY_CAPSULE_HEIGHT: f64 = 2.0;
/// Primary-color opacity of the empty-hour capsule.
pub const EMPTY_CAPSULE_OPACITY: f64 = 0.12;
/// Smallest bar height for an hour with activity.
pub const MIN_BAR_HEIGHT: f64 = 3.0;
/// Smallest visible slice height.
pub const MIN_SLICE_HEIGHT: f64 = 1.0;
/// Legend swatch size `(width, height)`.
pub const LEGEND_SWATCH: (f64, f64) = (8.0, 10.0);
/// Zen's opacity (Go uses the shared panel accent alpha).
pub const ZEN_ALPHA: f64 = 0.78;

/// Tooltip and slice title.
pub const fn series_title(series: OverviewSeries) -> &'static str {
    match series {
        OverviewSeries::Grok => "Grok",
        OverviewSeries::Grokbot => "Grokbot",
        OverviewSeries::Cursor => "Cursor",
        OverviewSeries::Claude => "Claude",
        OverviewSeries::OpenCodeGo => "OpenCode Go",
        OverviewSeries::OpenCodeZen => "OpenCode Zen",
    }
}

/// Short legend title (`Go`, `Zen` for OpenCode).
pub const fn series_legend_title(series: OverviewSeries) -> &'static str {
    match series {
        OverviewSeries::OpenCodeGo => "Go",
        OverviewSeries::OpenCodeZen => "Zen",
        other => series_title(other),
    }
}

const fn quota_label(series: OverviewSeries) -> &'static str {
    match series {
        OverviewSeries::Grok | OverviewSeries::Grokbot => "of weekly",
        OverviewSeries::Claude => "of 5-hour window",
        OverviewSeries::Cursor | OverviewSeries::OpenCodeGo | OverviewSeries::OpenCodeZen => {
            "of weekly share"
        }
    }
}

/// Fill color of a series: provider accents at 0.85, OpenCode Go orange at
/// 0.85 and OpenCode Zen purple at 0.78.
pub const fn series_color(series: OverviewSeries) -> Srgb {
    match series {
        OverviewSeries::Grok => ProviderId::Grok.panel_accent(),
        OverviewSeries::Grokbot => ProviderId::Grokbot.panel_accent(),
        OverviewSeries::Cursor => ProviderId::Cursor.panel_accent(),
        OverviewSeries::Claude => ProviderId::Claude.panel_accent(),
        OverviewSeries::OpenCodeGo => ProviderId::OpenCode.panel_accent(),
        OverviewSeries::OpenCodeZen => Srgb::rgb(0.58, 0.44, 0.86).with_alpha(ZEN_ALPHA),
    }
}

/// Hours shown on the x axis: `min(6, first active)..=max(22, last active)`,
/// or `6..=22` without activity.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::visible_hours;
/// assert_eq!(visible_hours(None), 6..=22);
/// assert_eq!(visible_hours(Some((3, 23))), 3..=23);
/// assert_eq!(visible_hours(Some((8, 10))), 6..=22);
/// ```
pub fn visible_hours(active: Option<(u8, u8)>) -> RangeInclusive<u8> {
    match active {
        Some((lo, hi)) => lo.min(6)..=hi.max(22),
        None => 6..=22,
    }
}

/// Height of a bar: 0 without activity, otherwise at least 3 and at most the
/// track.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::bar_height;
/// assert_eq!(bar_height(5.0, 10.0, 60.0), 30.0);
/// assert_eq!(bar_height(10.0, 10.0, 60.0), 60.0);
/// assert_eq!(bar_height(0.1, 10.0, 60.0), 3.0);
/// assert_eq!(bar_height(0.0, 10.0, 60.0), 0.0);
/// ```
pub fn bar_height(activity: f64, max: f64, track: f64) -> f64 {
    let fraction = if activity > 0.0 && max > 0.0 {
        (activity / max).min(1.0)
    } else {
        0.0
    };
    if fraction > 0.0 {
        track.min(MIN_BAR_HEIGHT.max(track * fraction))
    } else {
        0.0
    }
}

/// Splits `total` across slices proportionally to `acts`, lifting thin slices
/// to `minimum` (taking the difference from the tallest slice) when the
/// minimums fit.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::segment_heights;
/// assert_eq!(segment_heights(&[1.0, 3.0], 40.0, 1.0), vec![10.0, 30.0]);
/// ```
pub fn segment_heights(acts: &[f64], total: f64, minimum: f64) -> Vec<f64> {
    let clean = |a: f64| if a.is_finite() { a.max(0.0) } else { 0.0 };
    let sum: f64 = acts.iter().copied().map(clean).sum();
    if sum <= 0.0 || total <= 0.0 || !total.is_finite() {
        return vec![0.0; acts.len()];
    }
    let mut h: Vec<f64> = acts.iter().map(|a| clean(*a) / sum * total).collect();
    let shown: Vec<usize> = (0..h.len()).filter(|i| h[*i] > 0.0).collect();
    if shown.len() > 1 && minimum * shown.len() as f64 <= total {
        let mut lifted = 0.0;
        for &i in &shown {
            if h[i] < minimum {
                lifted += minimum - h[i];
                h[i] = minimum;
            }
        }
        take_from_surplus(&mut h, &shown, minimum, lifted);
    }
    h
}

/// Removes `lifted` from the slices in `shown`, never taking any below
/// `minimum`.
///
/// The tallest slice pays alone when it can (Swift's behaviour). When the
/// lifts are larger than its headroom, every slice above `minimum` pays in
/// proportion to its headroom instead. The headroom always covers `lifted`
/// because the minimums fit in the total, so no slice goes negative.
fn take_from_surplus(h: &mut [f64], shown: &[usize], minimum: f64, lifted: f64) {
    let tallest = shown
        .iter()
        .copied()
        .reduce(|best, i| if h[i] > h[best] { i } else { best })
        .expect("shown is not empty");
    if h[tallest] - lifted >= minimum {
        h[tallest] -= lifted;
        return;
    }
    let surplus: f64 = shown.iter().map(|&i| (h[i] - minimum).max(0.0)).sum();
    if surplus <= 0.0 {
        return;
    }
    let keep = (1.0 - lifted / surplus).max(0.0);
    for &i in shown {
        h[i] = minimum + (h[i] - minimum).max(0.0) * keep;
    }
}

/// Hours that get an axis label: both ends plus every multiple of 3 that is at
/// least 2 hours from both ends.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::axis_hours;
/// assert_eq!(axis_hours(&(6..=22)), vec![6, 9, 12, 15, 18, 22]);
/// assert_eq!(axis_hours(&(2..=23)), vec![2, 6, 9, 12, 15, 18, 21, 23]);
/// ```
pub fn axis_hours(range: &RangeInclusive<u8>) -> Vec<u8> {
    let (lower, upper) = (*range.start(), *range.end());
    range
        .clone()
        .filter(|h| {
            *h == lower
                || *h == upper
                || (h % 3 == 0 && h.saturating_sub(lower) >= 2 && upper.saturating_sub(*h) >= 2)
        })
        .collect()
}

/// Axis text: 12 for 0 and 12, `h-12` for 13..=23, `h` otherwise.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::axis_label_text;
/// assert_eq!(axis_label_text(0), "12");
/// assert_eq!(axis_label_text(15), "3");
/// assert_eq!(axis_label_text(9), "9");
/// ```
pub fn axis_label_text(hour: u8) -> String {
    match hour {
        0 | 12 => "12".to_owned(),
        13..=23 => (hour - 12).to_string(),
        h => h.to_string(),
    }
}

/// One drawn slice of a bar.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceModel {
    /// Which series.
    pub series: OverviewSeries,
    /// Height in points.
    pub height: f64,
    /// Distance from the bottom of the track in points.
    pub bottom: f64,
    /// Fill color.
    pub color: Srgb,
}

/// One bar of the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct BarModel {
    /// Hour of day.
    pub hour: u8,
    /// Total bar height in points; 0 for an empty hour.
    pub height: f64,
    /// Slices bottom to top; empty for an empty hour.
    pub slices: Vec<SliceModel>,
    /// `true` when a 2pt capsule at 12% primary opacity is drawn instead.
    pub empty_capsule: bool,
    /// Hover text; `None` for an empty hour.
    pub tooltip: Option<String>,
}

/// An x-axis label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisLabel {
    /// Hour of day.
    pub hour: u8,
    /// Text to draw.
    pub text: String,
}

/// A legend swatch with its title.
#[derive(Debug, Clone, PartialEq)]
pub struct LegendEntry {
    /// Which series.
    pub series: OverviewSeries,
    /// Short title (`Grok`, `Go`, `Zen`, ...).
    pub title: &'static str,
    /// Swatch color.
    pub color: Srgb,
}

/// Everything the panel needs to draw the daily chart.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartModel {
    /// Visible hour range.
    pub range: RangeInclusive<u8>,
    /// One bar per visible hour.
    pub bars: Vec<BarModel>,
    /// Axis labels.
    pub axis: Vec<AxisLabel>,
    /// Providers with activity in the visible hours, in stack order.
    pub legend: Vec<LegendEntry>,
}

fn active_span(usage: &ProviderDayHourlyUsage) -> Option<(u8, u8)> {
    let mut active = usage
        .hours
        .iter()
        .filter(|h| h.has_activity())
        .map(|h| h.hour);
    let first = active.next()?;
    Some((first, active.next_back().unwrap_or(first)))
}

/// Hover text: `"<hour>: <Title> 0.50% of weekly · 1.2M · ..."`.
pub fn tooltip_text(hour: &HourUsage) -> String {
    let segments: Vec<String> = OverviewSeries::STACK_ORDER
        .into_iter()
        .filter(|s| hour.deltas[s.index()] > 0.0)
        .map(|s| {
            let base = format!(
                "{} {:.2}% {}",
                series_title(s),
                hour.deltas[s.index()],
                quota_label(s)
            );
            match hour.tokens[s.index()] {
                Some(t) if t > 0 => format!("{base} · {}", format_tokens(t)),
                _ => base,
            }
        })
        .collect();
    format!("{}: {}", hour_label(hour.hour), segments.join(" · "))
}

fn build_bar(hour: &HourUsage, max: f64) -> BarModel {
    let height = bar_height(hour.activity, max, TRACK_HEIGHT);
    if height <= 0.0 {
        return BarModel {
            hour: hour.hour,
            height: 0.0,
            slices: Vec::new(),
            empty_capsule: true,
            tooltip: None,
        };
    }
    let shown: Vec<OverviewSeries> = OverviewSeries::STACK_ORDER
        .into_iter()
        .filter(|s| hour.deltas[s.index()] > 0.0)
        .collect();
    let acts: Vec<f64> = shown.iter().map(|s| hour.deltas[s.index()]).collect();
    let heights = segment_heights(&acts, height, MIN_SLICE_HEIGHT);
    let slices = shown
        .iter()
        .zip(&heights)
        .scan(0.0, |bottom, (s, h)| {
            let slice = SliceModel {
                series: *s,
                height: *h,
                bottom: *bottom,
                color: series_color(*s),
            };
            *bottom += h;
            Some(slice)
        })
        .collect();
    BarModel {
        hour: hour.hour,
        height,
        slices,
        empty_capsule: false,
        tooltip: Some(tooltip_text(hour)),
    }
}

impl ChartModel {
    /// Lays out the chart for a day. The bar maximum is taken over the visible
    /// hours.
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::NaiveDate;
    /// use tokenmon_core::overview::{ChartModel, OverviewDaySeries, OverviewSeries::Grok, ProviderDayHourlyUsage};
    /// let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    /// let mut w = vec![0.0; 24];
    /// w[9] = 2.0;
    /// let usage = ProviderDayHourlyUsage::build(day, &OverviewDaySeries::default().with_weights(Grok, w)).unwrap();
    /// let chart = ChartModel::build(&usage);
    /// assert_eq!(chart.range, 6..=22);
    /// assert_eq!(chart.bars.len(), 17);
    /// assert_eq!(chart.legend.len(), 1);
    /// ```
    pub fn build(usage: &ProviderDayHourlyUsage) -> Self {
        let range = visible_hours(active_span(usage));
        let visible: Vec<&HourUsage> = usage
            .hours
            .iter()
            .filter(|h| range.contains(&h.hour))
            .collect();
        let max = visible.iter().map(|h| h.activity).fold(0.0, f64::max);
        let bars = visible.iter().map(|h| build_bar(h, max)).collect();
        let axis = axis_hours(&range)
            .into_iter()
            .map(|hour| AxisLabel {
                hour,
                text: axis_label_text(hour),
            })
            .collect();
        let legend = OverviewSeries::STACK_ORDER
            .into_iter()
            .filter(|s| visible.iter().any(|h| h.deltas[s.index()] > 0.0))
            .map(|s| LegendEntry {
                series: s,
                title: series_legend_title(s),
                color: series_color(s),
            })
            .collect();
        Self {
            range,
            bars,
            axis,
            legend,
        }
    }
}

/// Card title: "Usage Today", "Usage Yesterday", else "Usage Wed, Oct 21".
///
/// # Examples
///
/// ```
/// use chrono::NaiveDate;
/// use tokenmon_core::overview::day_title;
/// let d = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
/// assert_eq!(day_title(0, d), "Usage Today");
/// assert_eq!(day_title(-1, d), "Usage Yesterday");
/// assert_eq!(day_title(-3, d), "Usage Wed, Oct 21");
/// ```
pub fn day_title(offset: i32, day: NaiveDate) -> String {
    match offset {
        0 => "Usage Today".to_owned(),
        -1 => "Usage Yesterday".to_owned(),
        _ => format!("Usage {}", day.format("%a, %b %-d")),
    }
}

/// Message shown when the day has no activity.
pub const fn empty_message(offset: i32) -> &'static str {
    if offset == 0 {
        "No Grok, Grokbot, OpenCode, Cursor, or Claude activity yet today."
    } else {
        "No activity recorded on this day."
    }
}

/// The day `offset` days from `today` (offsets are zero or negative).
pub fn day_for_offset(today: NaiveDate, offset: i32) -> NaiveDate {
    let days = Days::new(u64::from(offset.unsigned_abs()));
    if offset < 0 {
        today.checked_sub_days(days).unwrap_or(today)
    } else {
        today.checked_add_days(days).unwrap_or(today)
    }
}

/// `earliest != nil && start(earliest) < start(day)`.
pub fn can_go_back(day: NaiveDate, earliest: Option<NaiveDate>) -> bool {
    earliest.is_some_and(|e| e < day)
}

/// Navigation arrows are shown when `can_go_back || offset < 0`.
pub fn show_navigation(can_go_back: bool, offset: i32) -> bool {
    can_go_back || offset < 0
}

/// Offset after pressing Back (the caller enables Back only when
/// [`can_go_back`]).
pub fn previous_offset(offset: i32) -> i32 {
    offset.saturating_sub(1)
}

/// Offset after pressing Next: `min(0, offset + 1)`.
pub fn next_offset(offset: i32) -> i32 {
    offset.saturating_add(1).min(0)
}

/// First instant of `day` in `tz`; if midnight does not exist (DST gap), the
/// first valid instant after it.
fn start_of_day<Tz: TimeZone>(tz: &Tz, day: NaiveDate) -> DateTime<Utc> {
    let midnight = day.and_time(NaiveTime::MIN);
    (0..8)
        .find_map(|i| {
            let naive = midnight + chrono::Duration::minutes(30 * i);
            tz.from_local_datetime(&naive).earliest()
        })
        .map_or_else(
            || Utc.from_utc_datetime(&midnight),
            |d| d.with_timezone(&Utc),
        )
}

/// Eight local midnights starting at `date`'s own: when the Overview re-evaluates
/// "today". DST-safe because each is computed from its calendar day.
pub fn midnight_schedule<Tz: TimeZone>(tz: &Tz, date: NaiveDate) -> Vec<DateTime<Utc>> {
    (0..8u64)
        .map(|k| start_of_day(tz, date.checked_add_days(Days::new(k)).unwrap_or(date)))
        .collect()
}
