//! Per-hour activity model (`ProviderDayHourlyUsage`, spec 01 section 4.5).

use chrono::NaiveDate;

use super::series::{HOURS, OverviewDaySeries, OverviewSeries};

/// One hour of the day. Arrays are indexed by [`OverviewSeries::index`].
#[derive(Debug, Clone, PartialEq)]
pub struct HourUsage {
    /// Hour of day, 0..=23.
    pub hour: u8,
    /// Sum of the six deltas.
    pub activity: f64,
    /// Percentage points used this hour, `max(0, weight)`.
    pub deltas: [f64; 6],
    /// Each series' share of the hour's activity, 0..=100.
    pub shares: [f64; 6],
    /// Token counts; `None` for series that never report them.
    pub tokens: [Option<i64>; 6],
}

impl HourUsage {
    /// `true` when `activity > 0`.
    pub fn has_activity(&self) -> bool {
        self.activity > 0.0
    }
}

/// A day of hourly usage, always exactly 24 hours.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderDayHourlyUsage {
    /// Local calendar day.
    pub day: NaiveDate,
    /// The 24 hours.
    pub hours: Vec<HourUsage>,
    /// Largest hourly activity.
    pub max_activity: f64,
}

fn weight_at(series: &OverviewDaySeries, s: OverviewSeries, h: usize) -> f64 {
    let w = series
        .weights
        .get(&s)
        .and_then(|v| v.get(h))
        .copied()
        .unwrap_or(0.0);
    if w.is_finite() { w.max(0.0) } else { 0.0 }
}

fn tokens_at(series: &OverviewDaySeries, s: OverviewSeries, h: usize) -> Option<i64> {
    match s {
        OverviewSeries::Cursor | OverviewSeries::OpenCodeGo | OverviewSeries::OpenCodeZen => Some(
            series
                .tokens
                .get(&s)
                .and_then(|v| v.get(h))
                .copied()
                .unwrap_or(0)
                .max(0),
        ),
        // The Overview path never supplies Grok or Claude tokens, and Grokbot
        // never shows any.
        OverviewSeries::Grok | OverviewSeries::Claude | OverviewSeries::Grokbot => None,
    }
}

fn build_hour(series: &OverviewDaySeries, h: usize) -> HourUsage {
    let deltas = OverviewSeries::STACK_ORDER.map(|s| weight_at(series, s, h));
    let activity: f64 = OverviewSeries::SUM_ORDER
        .iter()
        .map(|s| deltas[s.index()])
        .sum();
    let shares = deltas.map(|d| {
        if activity > 0.0 {
            d / activity * 100.0
        } else {
            0.0
        }
    });
    let tokens = OverviewSeries::STACK_ORDER.map(|s| tokens_at(series, s, h));
    HourUsage {
        hour: h as u8,
        activity,
        deltas,
        shares,
        tokens,
    }
}

impl ProviderDayHourlyUsage {
    /// Builds the model, padding or truncating every input to 24 hours.
    /// Returns `None` when the day has no activity (the card shows its empty
    /// message).
    ///
    /// # Examples
    ///
    /// ```
    /// use chrono::NaiveDate;
    /// use tokenmon_core::overview::{OverviewDaySeries, OverviewSeries::Grok, ProviderDayHourlyUsage};
    /// let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    /// assert!(ProviderDayHourlyUsage::build(day, &OverviewDaySeries::default()).is_none());
    /// let s = OverviewDaySeries::default().with_weights(Grok, vec![0.0, 0.0, 2.0]);
    /// let u = ProviderDayHourlyUsage::build(day, &s).unwrap();
    /// assert_eq!(u.hours.len(), 24);
    /// assert_eq!(u.max_activity, 2.0);
    /// ```
    pub fn build(day: NaiveDate, series: &OverviewDaySeries) -> Option<Self> {
        let hours: Vec<HourUsage> = (0..HOURS).map(|h| build_hour(series, h)).collect();
        let max_activity = hours.iter().map(|h| h.activity).fold(0.0, f64::max);
        (max_activity > 0.0).then_some(Self {
            day,
            hours,
            max_activity,
        })
    }
}
