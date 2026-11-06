//! Overview series and building today's series from provider hourly data.
//!
//! Spec 01 sections 4.1 and 4.2.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hours in a day; every per-hour array is normalized to this length.
pub const HOURS: usize = 24;

/// One stacked series of the Overview chart.
///
/// The raw strings are persisted in the history file and must never change.
/// `Ord` follows the raw string so JSON maps keyed by series come out in the
/// same sorted-key order Swift's `.sortedKeys` produced.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::OverviewSeries;
/// assert_eq!(OverviewSeries::OpenCodeGo.to_string(), "openCodeGo");
/// assert_eq!("claude".parse::<OverviewSeries>().unwrap(), OverviewSeries::Claude);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverviewSeries {
    /// Grok, weekly allowance.
    #[serde(rename = "grok")]
    Grok,
    /// Grokbot, weekly allowance.
    #[serde(rename = "grokbot")]
    Grokbot,
    /// Cursor, weekly share of the monthly plan.
    #[serde(rename = "cursor")]
    Cursor,
    /// Claude, 5-hour window.
    #[serde(rename = "claude")]
    Claude,
    /// OpenCode Go, weekly share of the monthly plan.
    #[serde(rename = "openCodeGo")]
    OpenCodeGo,
    /// OpenCode Zen, weekly share of the monthly plan.
    #[serde(rename = "openCodeZen")]
    OpenCodeZen,
}

/// Returned when a string does not name an [`OverviewSeries`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown overview series: {0:?}")]
pub struct ParseOverviewSeriesError(pub String);

impl OverviewSeries {
    /// Every series in chart stack order, bottom to top.
    pub const STACK_ORDER: [Self; 6] = [
        Self::Grok,
        Self::Grokbot,
        Self::Cursor,
        Self::Claude,
        Self::OpenCodeGo,
        Self::OpenCodeZen,
    ];

    /// Order in which hourly deltas are summed into `activity` (spec 4.5).
    pub const SUM_ORDER: [Self; 6] = [
        Self::Grok,
        Self::OpenCodeGo,
        Self::OpenCodeZen,
        Self::Cursor,
        Self::Claude,
        Self::Grokbot,
    ];

    /// The persisted raw string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Grokbot => "grokbot",
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::OpenCodeGo => "openCodeGo",
            Self::OpenCodeZen => "openCodeZen",
        }
    }

    /// Position in [`Self::STACK_ORDER`]; index into per-series arrays.
    pub const fn index(self) -> usize {
        match self {
            Self::Grok => 0,
            Self::Grokbot => 1,
            Self::Cursor => 2,
            Self::Claude => 3,
            Self::OpenCodeGo => 4,
            Self::OpenCodeZen => 5,
        }
    }
}

impl fmt::Display for OverviewSeries {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OverviewSeries {
    type Err = ParseOverviewSeriesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::STACK_ORDER
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| ParseOverviewSeriesError(s.to_owned()))
    }
}

impl PartialOrd for OverviewSeries {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OverviewSeries {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// One day of per-hour activity for every series.
///
/// Weights are percentage points of each provider's own allowance per hour.
/// Arrays are normally 24 long but may differ (old files); [`Self::merging`]
/// pads with zero. Field order (`tokens`, `weights`) keeps JSON keys sorted.
///
/// # Examples
///
/// ```
/// use tokenmon_core::overview::{OverviewDaySeries, OverviewSeries};
/// let day = OverviewDaySeries::default().with_weights(OverviewSeries::Grok, vec![0.0, 2.0]);
/// assert!(day.has_activity());
/// assert!(!OverviewDaySeries::default().has_activity());
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OverviewDaySeries {
    /// Per-hour token counts, for the series that report them.
    #[serde(default)]
    pub tokens: BTreeMap<OverviewSeries, Vec<i64>>,
    /// Per-hour percentage points of the provider's allowance.
    #[serde(default)]
    pub weights: BTreeMap<OverviewSeries, Vec<f64>>,
}

fn merge_max<T: Copy + Default>(
    a: &BTreeMap<OverviewSeries, Vec<T>>,
    b: &BTreeMap<OverviewSeries, Vec<T>>,
    max: impl Fn(T, T) -> T,
) -> BTreeMap<OverviewSeries, Vec<T>> {
    let keys: BTreeSet<OverviewSeries> = a.keys().chain(b.keys()).copied().collect();
    keys.into_iter()
        .map(|k| {
            let merged = match (a.get(&k), b.get(&k)) {
                (Some(x), Some(y)) => (0..x.len().max(y.len()))
                    .map(|i| {
                        max(
                            x.get(i).copied().unwrap_or_default(),
                            y.get(i).copied().unwrap_or_default(),
                        )
                    })
                    .collect(),
                (Some(x), None) => x.clone(),
                (None, Some(y)) => y.clone(),
                (None, None) => Vec::new(),
            };
            (k, merged)
        })
        .collect()
}

impl OverviewDaySeries {
    /// Returns the series with `series` weights set (builder style).
    #[must_use]
    pub fn with_weights(self, series: OverviewSeries, weights: Vec<f64>) -> Self {
        let mut me = self;
        me.weights.insert(series, weights);
        me
    }

    /// Returns the series with `series` token counts set (builder style).
    #[must_use]
    pub fn with_tokens(self, series: OverviewSeries, tokens: Vec<i64>) -> Self {
        let mut me = self;
        me.tokens.insert(series, tokens);
        me
    }

    /// `true` when any weight is greater than zero.
    pub fn has_activity(&self) -> bool {
        self.weights.values().flatten().any(|w| *w > 0.0)
    }

    /// Per series and per hour, the larger value; shorter arrays are padded
    /// with zero and a series present on one side only is kept.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokenmon_core::overview::{OverviewDaySeries, OverviewSeries::Grok};
    /// let a = OverviewDaySeries::default().with_weights(Grok, vec![1.0, 5.0]);
    /// let b = OverviewDaySeries::default().with_weights(Grok, vec![3.0, 2.0, 4.0]);
    /// assert_eq!(a.merging(&b).weights[&Grok], vec![3.0, 5.0, 4.0]);
    /// ```
    #[must_use]
    pub fn merging(&self, other: &Self) -> Self {
        Self {
            tokens: merge_max(&self.tokens, &other.tokens, Ord::max),
            weights: merge_max(&self.weights, &other.weights, f64::max),
        }
    }

    /// Truncates every array to [`HOURS`] and replaces non-finite or negative
    /// weights with zero. Used on untrusted decoded data.
    #[must_use]
    pub fn normalized(self) -> Self {
        let weights = self
            .weights
            .into_iter()
            .map(|(k, v)| {
                let v = v
                    .into_iter()
                    .take(HOURS)
                    .map(|w| if w.is_finite() { w.max(0.0) } else { 0.0 })
                    .collect();
                (k, v)
            })
            .collect();
        let tokens = self
            .tokens
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().take(HOURS).map(|t| t.max(0)).collect()))
            .collect();
        Self { tokens, weights }
    }
}

/// A 24-hour weight array tagged with the day it belongs to
/// (`grokHourly`, `claudeHourly`, `grokbotHourly`).
#[derive(Debug, Clone, PartialEq)]
pub struct DayHourly {
    /// Local calendar day the values were recorded for.
    pub day: NaiveDate,
    /// Percentage points per hour.
    pub weights: Vec<f64>,
}

/// Cursor's `dayHourlyUsage`: quota weights and token weights.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorDay {
    /// Local calendar day.
    pub day: NaiveDate,
    /// `quotaHourWeights`, 24 values.
    pub quota_weights: Vec<f64>,
    /// `hourTokenWeights`, 24 values.
    pub hour_tokens: Vec<i64>,
}

/// OpenCode's `dayHourlyUsage` after the OpenCode slice has applied
/// `overviewProviderQuotaHourWeights(monthlyLimitUSD)` and
/// `overviewProviderHourTokenCounts()` for both products.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeDay {
    /// Local calendar day.
    pub day: NaiveDate,
    /// Go weights, 24 values.
    pub go_weights: Vec<f64>,
    /// Zen weights, 24 values.
    pub zen_weights: Vec<f64>,
    /// Go token counts, 24 values.
    pub go_tokens: Vec<i64>,
    /// Zen token counts, 24 values.
    pub zen_tokens: Vec<i64>,
}

/// Provider state needed to decide whether it contributes to the Overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderGate {
    /// The provider is enabled in settings.
    pub enabled: bool,
    /// A credential exists.
    pub signed_in: bool,
    /// The session needs the user to sign in again.
    pub needs_sign_in: bool,
}

impl ProviderGate {
    /// Enabled, signed in and not flagged for re-sign-in (every provider but
    /// OpenCode; Grokbot passes the Cursor session's gate).
    pub const fn contributes(self) -> bool {
        self.enabled && self.signed_in && !self.needs_sign_in
    }

    /// OpenCode needs only to be enabled.
    pub const fn contributes_without_sign_in(self) -> bool {
        self.enabled
    }
}

/// Everything [`build_today_series`] reads. Each field is `None` when the
/// provider does not contribute (disabled, signed out) or has no data; apply
/// [`ProviderGate`] before filling it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TodayInputs {
    /// `grokHourly`.
    pub grok: Option<DayHourly>,
    /// `claudeHourly`.
    pub claude: Option<DayHourly>,
    /// `grokbotHourly`.
    pub grokbot: Option<DayHourly>,
    /// `cursorPoller.dayHourlyUsage`.
    pub cursor: Option<CursorDay>,
    /// `openCodePoller.dayHourlyUsage`.
    pub open_code: Option<OpenCodeDay>,
}

fn sanitize(values: &[f64]) -> Vec<f64> {
    values
        .iter()
        .map(|w| if w.is_finite() { *w } else { 0.0 })
        .collect()
}

fn same_day_24<T>(day: NaiveDate, at: NaiveDate, values: &[T]) -> Option<&[T]> {
    (day == at && values.len() == HOURS).then_some(values)
}

fn add_hourly(
    acc: OverviewDaySeries,
    hourly: Option<&DayHourly>,
    series: OverviewSeries,
    date: NaiveDate,
) -> OverviewDaySeries {
    match hourly.and_then(|h| same_day_24(h.day, date, &h.weights)) {
        Some(w) => acc.with_weights(series, sanitize(w)),
        None => acc,
    }
}

fn add_cursor(
    acc: OverviewDaySeries,
    cursor: Option<&CursorDay>,
    date: NaiveDate,
) -> OverviewDaySeries {
    let Some(c) = cursor else { return acc };
    let acc = match same_day_24(c.day, date, &c.quota_weights) {
        Some(w) => acc.with_weights(OverviewSeries::Cursor, sanitize(w)),
        None => acc,
    };
    match same_day_24(c.day, date, &c.hour_tokens) {
        Some(t) => acc.with_tokens(OverviewSeries::Cursor, t.to_vec()),
        None => acc,
    }
}

/// `addOpenCode`: sets the Go and Zen weights and tokens when the data is for
/// `date` and every array has 24 hours.
fn add_open_code(
    acc: OverviewDaySeries,
    oc: Option<&OpenCodeDay>,
    date: NaiveDate,
) -> OverviewDaySeries {
    let Some(o) = oc.filter(|o| o.day == date) else {
        return acc;
    };
    let lens = [
        o.go_weights.len(),
        o.zen_weights.len(),
        o.go_tokens.len(),
        o.zen_tokens.len(),
    ];
    if lens.iter().any(|n| *n != HOURS) {
        return acc;
    }
    acc.with_weights(OverviewSeries::OpenCodeGo, sanitize(&o.go_weights))
        .with_weights(OverviewSeries::OpenCodeZen, sanitize(&o.zen_weights))
        .with_tokens(OverviewSeries::OpenCodeGo, o.go_tokens.clone())
        .with_tokens(OverviewSeries::OpenCodeZen, o.zen_tokens.clone())
}

/// Builds the live series for `date` (spec 4.2). A source contributes only
/// when it is for the same day and has exactly 24 values.
///
/// # Examples
///
/// ```
/// use chrono::NaiveDate;
/// use tokenmon_core::overview::{build_today_series, DayHourly, OverviewSeries, TodayInputs};
///
/// let today = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
/// let mut w = vec![0.0; 24];
/// w[9] = 1.5;
/// let inputs = TodayInputs {
///     grok: Some(DayHourly { day: today, weights: w }),
///     ..Default::default()
/// };
/// let s = build_today_series(&inputs, today);
/// assert_eq!(s.weights[&OverviewSeries::Grok][9], 1.5);
/// // Yesterday's data is ignored.
/// let stale = build_today_series(&inputs, today.succ_opt().unwrap());
/// assert!(!stale.has_activity());
/// ```
pub fn build_today_series(inputs: &TodayInputs, date: NaiveDate) -> OverviewDaySeries {
    let acc = OverviewDaySeries::default();
    let acc = add_hourly(acc, inputs.grok.as_ref(), OverviewSeries::Grok, date);
    let acc = add_hourly(acc, inputs.claude.as_ref(), OverviewSeries::Claude, date);
    let acc = add_hourly(acc, inputs.grokbot.as_ref(), OverviewSeries::Grokbot, date);
    let acc = add_cursor(acc, inputs.cursor.as_ref(), date);
    add_open_code(acc, inputs.open_code.as_ref(), date)
}
