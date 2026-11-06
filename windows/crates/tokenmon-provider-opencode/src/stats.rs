//! Local estimate: snapshot, hourly stacks and daily spends computed from
//! scanned rows (spec 03 sections 3.6 to 3.8).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike, Utc};

use crate::bounds::{Bounds, monthly_bounds, weekly_bounds};
use crate::db::{MessageRow, Scan};
use crate::pricing::{is_go_provider, is_plan_provider};
use crate::snapshot::{ModelUsage, OpenCodeSnapshot, WindowKind, WindowUsage};

/// Rolling window length.
pub const ROLLING_5H: Duration = Duration::seconds(18_000);

fn distinct_sessions<'a>(rows: impl Iterator<Item = &'a MessageRow>) -> u32 {
    let mut ids: HashSet<&str> = HashSet::new();
    let mut anonymous = 0_u32;
    for r in rows {
        if r.session_id.is_empty() {
            anonymous += 1;
        } else {
            ids.insert(&r.session_id);
        }
    }
    ids.len() as u32 + anonymous
}

fn go_window<'a>(
    kind: WindowKind,
    rows: &[&'a MessageRow],
    in_window: impl Fn(&MessageRow) -> bool,
    resets_at: impl Fn(&[&'a MessageRow]) -> Option<DateTime<Utc>>,
) -> WindowUsage {
    let hit: Vec<&MessageRow> = rows.iter().copied().filter(|r| in_window(r)).collect();
    WindowUsage {
        kind,
        used_usd: hit.iter().map(|r| r.billable().cost).sum(),
        limit_usd: kind.default_limit_usd(),
        resets_at: resets_at(&hit),
        session_count: distinct_sessions(hit.iter().copied()),
    }
}

/// Per-model usage over `rows` (already restricted to the window), plan
/// providers only. Reproduces the spec's `sessionCount` overwrite quirk.
pub fn model_usage(rows: &[&MessageRow]) -> Vec<ModelUsage> {
    let mut order: Vec<String> = Vec::new();
    let mut acc: HashMap<String, (ModelUsage, HashSet<&str>)> = HashMap::new();
    for r in rows.iter().filter(|r| is_plan_provider(&r.provider_id)) {
        let key = format!("{}/{}", r.provider_id, r.model_id);
        let b = r.billable();
        let (m, ids) = acc.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            (
                ModelUsage {
                    provider_id: r.provider_id.clone(),
                    model_id: r.model_id.clone(),
                    cost_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    session_count: 0,
                    percent_of_window: 0.0,
                    is_cost_estimated: false,
                    is_cost_unpriced: false,
                },
                HashSet::new(),
            )
        });
        m.cost_usd += b.cost;
        m.input_tokens = m.input_tokens.saturating_add(r.tokens.input);
        m.output_tokens = m.output_tokens.saturating_add(r.tokens.output);
        m.cache_read_tokens = m.cache_read_tokens.saturating_add(r.tokens.cache_read);
        m.cache_write_tokens = m.cache_write_tokens.saturating_add(r.tokens.cache_write);
        m.is_cost_estimated |= b.is_estimated;
        m.is_cost_unpriced |= b.is_unpriced;
        if r.session_id.is_empty() {
            m.session_count += 1;
        } else {
            ids.insert(&r.session_id);
            m.session_count = ids.len() as u32;
        }
    }
    let mut models: Vec<ModelUsage> = order
        .into_iter()
        .filter_map(|k| acc.remove(&k).map(|(m, _)| m))
        .filter(|m| {
            m.session_count > 0
                && (m.cost_usd > 0.0
                    || m.input_tokens > 0
                    || m.output_tokens > 0
                    || m.cache_read_tokens > 0
                    || m.cache_write_tokens > 0)
        })
        .collect();
    models.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then(b.output_tokens.cmp(&a.output_tokens))
    });
    let total: f64 = models.iter().map(|m| m.cost_usd).sum();
    for m in &mut models {
        m.percent_of_window = if total > 0.0 {
            (m.cost_usd / total * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
    }
    models
}

/// The local estimate (`OpenCodeLocalStats.snapshot`).
pub fn snapshot(scan: &Scan, now: DateTime<Utc>) -> OpenCodeSnapshot {
    let week = weekly_bounds(now);
    let month = monthly_bounds(now, scan.subscribed_at.unwrap_or(now));
    let go: Vec<&MessageRow> = scan
        .rows
        .iter()
        .filter(|r| is_go_provider(&r.provider_id))
        .collect();
    let rolling_start = now - ROLLING_5H;
    let rolling = go_window(
        WindowKind::Rolling5h,
        &go,
        |r| r.date >= rolling_start,
        |hit| hit.iter().map(|r| r.date).max().map(|d| d + ROLLING_5H),
    );
    let weekly = go_window(WindowKind::Weekly, &go, |r| week.contains(r.date), |_| {
        Some(week.end)
    });
    let monthly = go_window(WindowKind::Monthly, &go, |r| month.contains(r.date), |_| {
        Some(month.end)
    });
    let week_rows: Vec<&MessageRow> = scan.rows.iter().filter(|r| week.contains(r.date)).collect();
    let month_plan = scan
        .rows
        .iter()
        .filter(|r| month.contains(r.date) && is_plan_provider(&r.provider_id));
    let (mut tokens, mut input, mut output, mut usd) = (0_i64, 0_i64, 0_i64, 0.0_f64);
    for r in month_plan {
        tokens = tokens.saturating_add(r.tokens.total());
        input = input.saturating_add(r.tokens.input);
        output = output.saturating_add(r.tokens.output);
        usd += r.billable().cost;
    }
    OpenCodeSnapshot {
        windows: vec![rolling, weekly, monthly],
        models: model_usage(&week_rows),
        is_estimated: true,
        monthly_tokens: tokens,
        monthly_estimated_usd: usd,
        monthly_input_tokens: input,
        monthly_output_tokens: output,
    }
}

/// Spend per local calendar day from Go rows' billable cost; rows with a
/// non-positive cost are skipped.
pub fn daily_spends_by_day(rows: &[&MessageRow], offset: FixedOffset) -> BTreeMap<NaiveDate, f64> {
    let mut out = BTreeMap::new();
    for r in rows {
        let cost = r.billable().cost;
        if cost > 0.0 {
            *out.entry(r.date.with_timezone(&offset).date_naive())
                .or_insert(0.0) += cost;
        }
    }
    out
}

/// Go rows inside `bounds`.
pub fn go_rows_in(scan: &Scan, bounds: Bounds) -> Vec<&MessageRow> {
    scan.rows
        .iter()
        .filter(|r| is_go_provider(&r.provider_id) && bounds.contains(r.date))
        .collect()
}

/// Month spends: Go rows inside the subscription month containing `now`.
pub fn month_daily_spends(
    scan: &Scan,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> BTreeMap<NaiveDate, f64> {
    let month = monthly_bounds(now, scan.subscribed_at.unwrap_or(now));
    daily_spends_by_day(&go_rows_in(scan, month), offset)
}

/// Scales spends so they sum to `used`; unchanged when either the sum or
/// `used` is not positive.
pub fn scaled_spends_percent(
    spends: &BTreeMap<NaiveDate, f64>,
    used: f64,
) -> BTreeMap<NaiveDate, f64> {
    let sum: f64 = spends.values().sum();
    if sum > 0.0 && used > 0.0 {
        let k = used / sum;
        spends.iter().map(|(d, v)| (*d, v * k)).collect()
    } else {
        spends.clone()
    }
}

/// Spreads `used_percent` evenly over the elapsed days of the period when no
/// local spends exist (step 4 of `monthDailyBudgetDays`).
pub fn spread_used_percent(
    period_start: NaiveDate,
    today: NaiveDate,
    used_percent: f64,
) -> BTreeMap<NaiveDate, f64> {
    let elapsed = (today - period_start).num_days().saturating_add(1).max(1);
    (0..elapsed)
        .filter_map(|i| period_start.checked_add_signed(Duration::days(i)))
        .map(|d| (d, used_percent / elapsed as f64))
        .collect()
}

/// One model's share of one hour.
#[derive(Debug, Clone, PartialEq)]
pub struct HourSegment {
    /// Provider id.
    pub provider_id: String,
    /// Model id.
    pub model_id: String,
    /// Recorded cost.
    pub cost_usd: f64,
    /// Billable (quota) cost.
    pub quota_cost: f64,
    /// All tokens.
    pub total_tokens: i64,
    /// Message count.
    pub messages: u32,
}

/// One hour of the day.
#[derive(Debug, Clone, PartialEq)]
pub struct HourUsage {
    /// Local hour 0 to 23.
    pub hour: u32,
    /// Segments sorted by cost then messages, descending.
    pub segments: Vec<HourSegment>,
    /// Messages in the hour.
    pub message_count: u32,
}

impl HourUsage {
    /// Sum of recorded costs.
    pub fn total_usd(&self) -> f64 {
        self.segments.iter().map(|s| s.cost_usd).sum()
    }

    /// `12a`, `1a` .. `11a`, `12p`, `1p` .. `11p`.
    pub fn label(&self) -> String {
        let h12 = if self.hour.is_multiple_of(12) { 12 } else { self.hour % 12 };
        format!("{h12}{}", if self.hour < 12 { 'a' } else { 'p' })
    }
}

/// Legend entry of the hourly chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegendEntry {
    /// `provider/model`.
    pub id: String,
    /// Model id.
    pub label: String,
}

/// Today's hourly stacks.
#[derive(Debug, Clone, PartialEq)]
pub struct DayHourlyUsage {
    /// Always 24 entries.
    pub hours: Vec<HourUsage>,
    /// Top models by recorded cost, at most the requested count.
    pub legend: Vec<LegendEntry>,
}

impl DayHourlyUsage {
    /// No cost and no messages in any hour.
    pub fn is_empty(&self) -> bool {
        self.hours.iter().all(|h| h.message_count == 0 && h.total_usd() <= 0.0)
    }
}

/// `buildDayHourlyUsage`: all providers, local day containing `now`.
pub fn build_day_hourly_usage(
    rows: &[MessageRow],
    now: DateTime<Utc>,
    offset: FixedOffset,
    max_legend_models: usize,
) -> DayHourlyUsage {
    let today = now.with_timezone(&offset).date_naive();
    let mut buckets: Vec<HashMap<String, HourSegment>> = (0..24).map(|_| HashMap::new()).collect();
    let mut counts = [0_u32; 24];
    let mut totals: HashMap<String, f64> = HashMap::new();
    for r in rows {
        let local = r.date.with_timezone(&offset);
        if local.date_naive() != today {
            continue;
        }
        let h = local.hour() as usize;
        let key = format!("{}/{}", r.provider_id, r.model_id);
        let seg = buckets[h].entry(key.clone()).or_insert_with(|| HourSegment {
            provider_id: r.provider_id.clone(),
            model_id: r.model_id.clone(),
            cost_usd: 0.0,
            quota_cost: 0.0,
            total_tokens: 0,
            messages: 0,
        });
        seg.cost_usd += r.cost_usd;
        seg.quota_cost += r.billable().cost;
        seg.total_tokens = seg.total_tokens.saturating_add(r.tokens.total());
        seg.messages += 1;
        counts[h] += 1;
        *totals.entry(key).or_insert(0.0) += r.cost_usd;
    }
    let hours = buckets
        .into_iter()
        .enumerate()
        .map(|(h, b)| {
            let mut segments: Vec<HourSegment> =
                b.into_values().filter(|s| s.cost_usd > 0.0 || s.messages > 0).collect();
            segments.sort_by(|a, b| {
                b.cost_usd
                    .total_cmp(&a.cost_usd)
                    .then(b.messages.cmp(&a.messages))
                    .then_with(|| (&a.provider_id, &a.model_id).cmp(&(&b.provider_id, &b.model_id)))
            });
            HourUsage {
                hour: h as u32,
                segments,
                message_count: counts[h],
            }
        })
        .collect();
    let mut legend: Vec<(String, f64)> = totals.into_iter().collect();
    legend.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    DayHourlyUsage {
        hours,
        legend: legend
            .into_iter()
            .take(max_legend_models)
            .map(|(id, _)| LegendEntry {
                label: id.split_once('/').map_or_else(|| id.clone(), |(_, m)| m.to_owned()),
                id,
            })
            .collect(),
    }
}

/// Per-hour Go and Zen series for the Overview chart.
#[derive(Debug, Clone, PartialEq)]
pub struct HourSeries {
    /// Go (`opencode-go`) values by hour.
    pub go: [f64; 24],
    /// Zen (`opencode`) values by hour.
    pub zen: [f64; 24],
}

fn split_series(day: &DayHourlyUsage, value: impl Fn(&HourSegment) -> f64) -> HourSeries {
    let mut s = HourSeries {
        go: [0.0; 24],
        zen: [0.0; 24],
    };
    for h in &day.hours {
        for seg in &h.segments {
            match seg.provider_id.as_str() {
                "opencode-go" => s.go[h.hour as usize] += value(seg),
                "opencode" => s.zen[h.hour as usize] += value(seg),
                _ => {}
            }
        }
    }
    s
}

/// `overviewProviderQuotaHourWeights`: quota cost as a percent of the weekly
/// share of the monthly limit. All zeros when that share is not positive.
pub fn overview_quota_hour_weights(day: &DayHourlyUsage, monthly_limit_usd: f64) -> HourSeries {
    let weekly = monthly_limit_usd / (365.2425 / 12.0 / 7.0);
    if weekly <= 0.0 || !weekly.is_finite() {
        return HourSeries {
            go: [0.0; 24],
            zen: [0.0; 24],
        };
    }
    split_series(day, |s| s.quota_cost.max(0.0) / weekly * 100.0)
}

/// `overviewProviderHourTokenCounts`.
pub fn overview_hour_token_counts(day: &DayHourlyUsage) -> HourSeries {
    split_series(day, |s| s.total_tokens as f64)
}
