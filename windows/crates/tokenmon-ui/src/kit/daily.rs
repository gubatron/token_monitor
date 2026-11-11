//! View model for the daily bar charts: turns per-day budget data into the
//! pre-formatted [`DayBar`](crate::kit::ui::DayBar) structs the Slint charts display.

use chrono::{Datelike, NaiveDate};
use slint::{ModelRc, SharedString, VecModel};

use crate::kit::hover::{DayUsage, usage_line};
use crate::kit::ui::{DayBar, HoverCardData, LabelValue};

/// Share of a day's allowance below which no fill is drawn.
const MIN_FILL_FRACTION: f64 = 0.005;

/// Pre-formatted token details shown in the hover label (the caller owns
/// `Format.tokens` / `Format.usd`, which live in the core slice).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DayTokensText {
    /// Total tokens, e.g. `1.2M`. Empty means "no token section".
    pub total: String,
    /// Words after the total (`tokens`, `tokens on this PC`); empty reads
    /// `tokens`.
    pub unit: String,
    /// Cost with prefix/suffix applied, e.g. `≈ $4.20+`; empty hides it.
    pub cost: String,
    /// Input tokens.
    pub input: String,
    /// Output tokens.
    pub output: String,
    /// Cache-read tokens.
    pub cache_read: String,
    /// Cache-write tokens.
    pub cache_write: String,
    /// Up to three `(model name, tokens)` rows.
    pub models: Vec<(String, String)>,
}

/// One day of a daily budget chart (`DailyBudgetDay`).
#[derive(Debug, Clone, PartialEq)]
pub struct DailyBudgetDay {
    /// Local calendar date.
    pub date: NaiveDate,
    /// Percentage of the period pool spent (`spentUSD`).
    pub spent: f64,
    /// The day's allowance as a percentage of the pool (`budgetUSD`).
    pub budget: f64,
    /// Spent as a percentage of the allowance (bar height driver).
    pub percent_of_budget: f64,
    /// Whether the day exceeded its allowance.
    pub is_over_budget: bool,
    /// Whether the day belongs to the pool before an early provider reset.
    pub is_prior_window: bool,
    /// Token details for the hover, when known.
    pub tokens: Option<DayTokensText>,
}

/// Two-letter weekday label (`Mo`, `Tu`, ...): the first two characters of the
/// English abbreviated weekday.
pub fn weekday_label(date: NaiveDate) -> String {
    date.format("%a").to_string().chars().take(2).collect()
}

/// Builds the Slint struct for one column. `noun` is `"weekly"` / `"monthly"`.
pub fn day_bar(day: &DailyBudgetDay, today: NaiveDate, noun: &str) -> DayBar {
    let future = day.date > today;
    let show_fill = day.percent_of_budget / 100.0 > MIN_FILL_FRACTION && !future;
    let usage = if future {
        String::new()
    } else {
        usage_line(
            DayUsage {
                spent: day.spent,
                budget: day.budget,
                is_prior_window: day.is_prior_window,
            },
            noun,
        )
    };
    let tokens = day.tokens.clone().unwrap_or_default();
    let rows = |pairs: Vec<(&str, String)>| -> ModelRc<LabelValue> {
        label_values(pairs.into_iter().map(|(l, v)| (l.to_owned(), v)))
    };
    DayBar {
        fill_percent: day.percent_of_budget as f32,
        show_fill,
        prior_window: day.is_prior_window,
        future,
        label_percent: SharedString::from(format!("{}%", (day.spent.round() as i64).max(0))),
        label_weekday: weekday_label(day.date).into(),
        hover: HoverCardData {
            title: day.date.format("%A, %b %-d").to_string().into(),
            caption: usage.into(),
            total: tokens.total.clone().into(),
            unit: tokens.unit.clone().into(),
            value: tokens.cost.clone().into(),
            rows: rows(vec![
                ("Input", tokens.input),
                ("Output", tokens.output),
                ("Cache read", tokens.cache_read),
                ("Cache write", tokens.cache_write),
            ]),
            extra_rows: label_values(tokens.models.into_iter().take(3)),
            note: SharedString::new(),
            verdict: SharedString::new(),
        },
    }
}

/// A model of label/value rows.
fn label_values(pairs: impl Iterator<Item = (String, String)>) -> ModelRc<LabelValue> {
    let items: Vec<LabelValue> = pairs
        .map(|(l, v)| LabelValue {
            label: l.into(),
            value: v.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

/// Builds every column of a chart.
pub fn day_bars(days: &[DailyBudgetDay], today: NaiveDate, noun: &str) -> ModelRc<DayBar> {
    let bars: Vec<DayBar> = days.iter().map(|d| day_bar(d, today, noun)).collect();
    ModelRc::new(VecModel::from(bars))
}

/// Header range label: `October 13 – 19` within a month, else
/// `September 29 – Oct 5`.
///
/// # Examples
///
/// ```
/// use chrono::NaiveDate;
/// use tokenmon_ui::kit::daily::range_label;
/// let d = |m, day| NaiveDate::from_ymd_opt(2025, m, day).unwrap();
/// assert_eq!(range_label(d(10, 13), d(10, 19)), "October 13 – 19");
/// assert_eq!(range_label(d(9, 29), d(10, 5)), "September 29 – Oct 5");
/// ```
pub fn range_label(first: NaiveDate, last: NaiveDate) -> String {
    let start = first.format("%B %-d");
    if first.month() == last.month() && first.year() == last.year() {
        format!("{start} – {}", last.day())
    } else {
        format!("{start} – {}", last.format("%b %-d"))
    }
}

/// The `14.3%` figure next to the header; empty when the budget is not positive.
pub fn header_percent(daily_budget: f64) -> String {
    if daily_budget > 0.0 {
        format!("{daily_budget:.1}%")
    } else {
        String::new()
    }
}

/// Footer text for the live (current-week) chart. `pace_caption` is
/// `DailyBudget.paceCaption(pace)` when a pace could be computed.
pub fn footer_text(
    days: &[DailyBudgetDay],
    today: NaiveDate,
    pace_caption: Option<String>,
    shows_live_footer: bool,
) -> String {
    if !shows_live_footer {
        return String::new();
    }
    if let Some(caption) = pace_caption {
        return caption;
    }
    let budget = days.first().map_or(0.0, |d| d.budget);
    if days.iter().all(|d| d.spent <= 0.001) {
        return format!("No usage yet · {budget:.1}%/day");
    }
    if days.iter().any(|d| d.date == today && d.is_over_budget) {
        return "Over today's allowance".to_owned();
    }
    String::new()
}
