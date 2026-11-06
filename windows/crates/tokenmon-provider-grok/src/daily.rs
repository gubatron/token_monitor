//! Billing-period daily bars (spec 02 sections 1.3.1 and 1.3.2).
//!
//! The chart axis is the provider's billing period (7 calendar days, 8 on the
//! reset morning), anchored to `resetsAt`. Bars are local day-over-day
//! deltas of the stored snapshots; there is no chart without a provider reset
//! and a calendar week is never substituted.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::calendar::{DayCalendar, seconds_between};
use crate::models::{
    DailyUsageDay, DailyUsageLegendItem, DailyUsageSegment, DailyUsageWeek, ProductCatalog,
    ProductColor, ProductUsage, WeeklyUsageSnapshot,
};

/// Most whole periods the stale-reset roll-forward advances.
const MAX_ROLL_FORWARD_PERIODS: u32 = 520;
/// Smallest used% (or product share) that counts as usage.
const NOISE_FLOOR: f64 = 0.05;
/// Reset instants further apart than this are different billing periods.
const PERIOD_ADVANCE_SECONDS: f64 = 12.0 * 3600.0;
/// A used% drop larger than this without a period change is a server rebase.
const REBASE_DROP: f64 = 5.0;

type EndOfDay<'a> = BTreeMap<DateTime<Utc>, &'a WeeklyUsageSnapshot>;

/// Calendar days of the active billing period `(start, end)`.
///
/// Before `resets_at`, the window is the previous reset day through the day
/// before the reset; once it fires, the window rolls to the new period
/// starting that day (and by whole periods when the provider lags). On the
/// reset morning, the running period still owns today, so the current window
/// (`week_offset == 0`) runs through the reset day as an eighth bar. `None`
/// without `resets_at`. The roll-forward loop is capped at 520 iterations.
///
/// # Examples
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::ManualClock;
/// use tokenmon_provider_grok::calendar::LocalCalendar;
/// use tokenmon_provider_grok::daily::billing_period_week_bounds;
///
/// let clock = ManualClock::from_unix(0);
/// let cal = LocalCalendar::new(&clock);
/// let reset = Utc.with_ymd_and_hms(2026, 7, 16, 18, 57, 0).unwrap();
/// let now = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
/// let (start, end) = billing_period_week_bounds(Some(reset), 0, &cal, now).unwrap();
/// assert_eq!(start, Utc.with_ymd_and_hms(2026, 7, 9, 0, 0, 0).unwrap());
/// assert_eq!(end, Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap());
/// ```
pub fn billing_period_week_bounds(
    resets_at: Option<DateTime<Utc>>,
    week_offset: i64,
    cal: &dyn DayCalendar,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let resets_at = resets_at?;
    let reset_day = cal.start_of_day(resets_at);
    let base_start = if now >= resets_at {
        roll_forward_to_today(reset_day, cal, now)
    } else {
        cal.add_days(reset_day, -7)
    };
    let shifted_start = cal.add_days(base_start, week_offset.saturating_mul(7));
    let shifted_end = cal.add_days(shifted_start, 6);
    if week_offset == 0 && now < resets_at && cal.same_day(now, reset_day) {
        return Some((shifted_start, reset_day));
    }
    Some((shifted_start, shifted_end))
}

/// The provider can lag `resets_at` by more than one period, leaving the
/// window in the past: roll forward by whole periods until it contains today.
fn roll_forward_to_today(
    reset_day: DateTime<Utc>,
    cal: &dyn DayCalendar,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let today = cal.start_of_day(now);
    let mut base = reset_day;
    for _ in 0..MAX_ROLL_FORWARD_PERIODS {
        if today <= cal.add_days(base, 6) {
            break;
        }
        let advanced = cal.add_days(base, 7);
        if advanced <= base {
            break;
        }
        base = advanced;
    }
    base
}

fn period_advanced(from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> bool {
    match (from, to) {
        (Some(from), Some(to)) => seconds_between(from, to) > PERIOD_ADVANCE_SECONDS,
        _ => false,
    }
}

fn same_billing_period(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> bool {
    !period_advanced(a, b) && !period_advanced(b, a)
}

/// `history` (oldest first) plus `current` when it is newer and not a
/// same-day near-duplicate of the last sample, sorted by time.
fn merged_samples<'a>(
    history: &'a [WeeklyUsageSnapshot],
    current: Option<&'a WeeklyUsageSnapshot>,
    cal: &dyn DayCalendar,
) -> Vec<&'a WeeklyUsageSnapshot> {
    let mut samples: Vec<&WeeklyUsageSnapshot> = history.iter().collect();
    if let Some(current) = current {
        let newer = samples
            .last()
            .is_none_or(|last| last.fetched_at < current.fetched_at);
        let near_duplicate = samples.last().is_some_and(|last| {
            (last.used_percent - current.used_percent).abs() < NOISE_FLOOR
                && seconds_between(last.fetched_at, current.fetched_at).abs() < 60.0
                && cal.same_day(last.fetched_at, current.fetched_at)
        });
        if newer && !near_duplicate {
            samples.push(current);
        }
    }
    samples.sort_by_key(|s| s.fetched_at);
    samples
}

/// The last sample of each local day, preferring post-rollover samples when
/// `resetsAt` advances mid-day.
fn end_of_day_samples<'a>(
    samples: &[&'a WeeklyUsageSnapshot],
    cal: &dyn DayCalendar,
) -> EndOfDay<'a> {
    let mut by_day: EndOfDay<'a> = BTreeMap::new();
    for sample in samples {
        let day = cal.start_of_day(sample.fetched_at);
        let Some(existing) = by_day.get(&day) else {
            by_day.insert(day, sample);
            continue;
        };
        if period_advanced(existing.resets_at, sample.resets_at)
            && sample.fetched_at >= existing.fetched_at
        {
            by_day.insert(day, sample);
        } else if period_advanced(sample.resets_at, existing.resets_at) {
            // An older-period sample after a newer period already won the day.
        } else if sample.fetched_at >= existing.fetched_at {
            by_day.insert(day, sample);
        }
    }
    by_day
}

/// The `resetsAt` of the period that owns the displayed window: the live value
/// for the current week, in-window sample metadata for past weeks (falling
/// back to the day after `week_end`).
fn period_anchor_resets(
    week_offset: i64,
    week_end: DateTime<Utc>,
    current_resets_at: Option<DateTime<Utc>>,
    samples_in_week: &[DateTime<Utc>],
    end_of_day: &EndOfDay<'_>,
    cal: &dyn DayCalendar,
) -> Option<DateTime<Utc>> {
    let from_history = samples_in_week
        .last()
        .and_then(|d| end_of_day.get(d))
        .and_then(|s| s.resets_at);
    if week_offset == 0 {
        return current_resets_at.or(from_history);
    }
    if from_history.is_some() {
        return from_history;
    }
    let expected = cal.add_days(week_end, 1);
    match current_resets_at {
        Some(live) if cal.same_day(cal.start_of_day(live), expected) => Some(live),
        _ => Some(cal.noon_of_day(expected)),
    }
}

/// Latest day whose used% fell sharply without `resetsAt` advancing (a server
/// rebase).
fn latest_mid_period_recalibration_day(
    sample_days: &[DateTime<Utc>],
    end_of_day: &EndOfDay<'_>,
) -> Option<DateTime<Utc>> {
    sample_days
        .windows(2)
        .filter_map(|pair| {
            let (prev, day) = (end_of_day.get(&pair[0])?, end_of_day.get(&pair[1])?);
            let dropped = day.used_percent + REBASE_DROP < prev.used_percent;
            (dropped && !period_advanced(prev.resets_at, day.resets_at)).then_some(pair[1])
        })
        .last()
}

struct WeekSamples<'a> {
    end_of_day: EndOfDay<'a>,
    sorted_days: Vec<DateTime<Utc>>,
    valid_sample_days: Vec<DateTime<Utc>>,
    tracking_start_day: Option<DateTime<Utc>>,
    week_start: DateTime<Utc>,
}

/// Builds the daily-usage week for the billing period.
///
/// `history` is oldest first (the active account's rows). `resets_at` is the
/// provider's reset instant; it falls back to `current.resets_at`. Returns
/// `None` when neither exists. `week_offset` is `0` for the current period,
/// negative for earlier ones.
pub fn week(
    history: &[WeeklyUsageSnapshot],
    current: Option<&WeeklyUsageSnapshot>,
    week_offset: i64,
    resets_at: Option<DateTime<Utc>>,
    cal: &dyn DayCalendar,
    now: DateTime<Utc>,
) -> Option<DailyUsageWeek> {
    let effective_resets_at = resets_at.or_else(|| current.and_then(|c| c.resets_at));
    let (week_start, week_end) =
        billing_period_week_bounds(effective_resets_at, week_offset, cal, now)?;
    let day_count = (cal.days_between(week_start, week_end) + 1).max(1);

    let merged = merged_samples(history, current, cal);
    let end_of_day = end_of_day_samples(&merged, cal);
    let sorted_days: Vec<DateTime<Utc>> = end_of_day.keys().copied().collect();
    let through_week_end: Vec<DateTime<Utc>> =
        sorted_days.iter().copied().filter(|d| *d <= week_end).collect();
    let in_week: Vec<DateTime<Utc>> = through_week_end
        .iter()
        .copied()
        .filter(|d| *d >= week_start)
        .collect();

    let anchor = period_anchor_resets(
        week_offset,
        week_end,
        current
            .and_then(|c| c.resets_at)
            .or(effective_resets_at),
        &in_week,
        &end_of_day,
        cal,
    );
    let same_period: Vec<DateTime<Utc>> = through_week_end
        .iter()
        .copied()
        .filter(|d| same_billing_period(end_of_day.get(d).and_then(|s| s.resets_at), anchor))
        .collect();
    let tracking_start_day = latest_mid_period_recalibration_day(&same_period, &end_of_day);
    let valid_sample_days: Vec<DateTime<Utc>> = match tracking_start_day {
        Some(start) => same_period.iter().copied().filter(|d| *d >= start).collect(),
        None => same_period,
    };

    let in_period_days = valid_sample_days
        .iter()
        .filter(|d| **d >= week_start && **d <= week_end)
        .count();
    let samples = WeekSamples {
        end_of_day,
        sorted_days,
        valid_sample_days,
        tracking_start_day,
        week_start,
    };
    let days = (0..day_count)
        .map(|offset| {
            let day_start = cal.add_days(week_start, offset);
            let bar = day_bar(day_start, &samples, cal);
            make_day(day_start, bar, cal, now)
        })
        .collect();
    let is_estimated = in_period_days < 2 && week_offset == 0;
    Some(finalize(week_start, week_end, days, is_estimated))
}

pub(crate) fn make_day(
    day_start: DateTime<Utc>,
    bar: Bar,
    cal: &dyn DayCalendar,
    now: DateTime<Utc>,
) -> DailyUsageDay {
    let date = cal.local_date(day_start);
    DailyUsageDay {
        day_start,
        weekday_symbol: date.format("%a").to_string().chars().take(3).collect(),
        day_of_month: date.format("%-d").to_string(),
        segments: bar.segments,
        is_today: cal.same_day(day_start, now),
        is_after_reset: bar.is_after_reset,
    }
}

/// Segments and the after-reset flag of one day.
pub(crate) struct Bar {
    pub(crate) segments: Vec<DailyUsageSegment>,
    pub(crate) is_after_reset: bool,
}

fn day_bar(day_start: DateTime<Utc>, samples: &WeekSamples<'_>, cal: &dyn DayCalendar) -> Bar {
    let end_of_day = &samples.end_of_day;
    let is_period_start = cal.same_day(day_start, samples.week_start);
    let day_resets = end_of_day.get(&day_start).and_then(|s| s.resets_at);
    let prev_day = samples.valid_sample_days.iter().rev().find(|prev| {
        **prev < day_start
            && same_billing_period(end_of_day.get(prev).and_then(|s| s.resets_at), day_resets)
    });
    let prev_snapshot = prev_day.and_then(|d| end_of_day.get(d));
    let is_tracking_start = samples
        .tracking_start_day
        .is_some_and(|d| cal.same_day(day_start, d));
    let mut bar = Bar {
        segments: Vec::new(),
        is_after_reset: is_tracking_start,
    };

    let Some(day_snapshot) = end_of_day.get(&day_start) else {
        return bar;
    };
    if !samples.valid_sample_days.contains(&day_start) {
        return bar;
    }
    let day_cumulative = day_snapshot.used_percent;
    // The day's own products, even when empty (Swift's optional chaining
    // never falls through to the current snapshot once the day has a sample).
    let products: &[ProductUsage] = &day_snapshot.products;
    let previous_products: &[ProductUsage] = prev_snapshot.map_or(&[], |s| &s.products);

    if let Some(previous) = prev_snapshot.map(|s| s.used_percent) {
        if day_cumulative + REBASE_DROP < previous {
            // Mid-period rebase drop: restart with absolute week-to-date.
            bar.is_after_reset = true;
            if day_cumulative > NOISE_FLOOR {
                bar.segments = segments_from_products(products, Some(day_cumulative));
            }
        } else {
            bar.segments = growth_segments(previous, day_cumulative, previous_products, products);
        }
        return bar;
    }

    let prior_any = |lower: Option<DateTime<Utc>>| {
        samples
            .sorted_days
            .iter()
            .rev()
            .find(|d| **d < day_start && lower.is_none_or(|l| **d >= l))
            .and_then(|d| end_of_day.get(d))
    };
    let rolled_over = |snap: Option<&&WeeklyUsageSnapshot>| {
        snap.is_some_and(|s| period_advanced(s.resets_at, day_resets))
    };
    if day_cumulative > NOISE_FLOOR && (is_tracking_start || is_period_start) {
        if is_tracking_start || rolled_over(prior_any(None)) {
            bar.is_after_reset = true;
        }
        bar.segments = segments_from_products(products, Some(day_cumulative));
    } else if day_cumulative > NOISE_FLOOR
        && rolled_over(prior_any(Some(samples.week_start)))
    {
        // Rollover observed between two days of this window: paint the
        // post-reset week-to-date only.
        bar.is_after_reset = true;
        bar.segments = segments_from_products(products, Some(day_cumulative));
    }
    // First sample mid-period: the bar stays empty until a second day.
    bar
}

pub(crate) fn finalize(
    week_start: DateTime<Utc>,
    week_end: DateTime<Utc>,
    days: Vec<DailyUsageDay>,
    is_estimated: bool,
) -> DailyUsageWeek {
    let mut legend: Vec<DailyUsageLegendItem> = Vec::new();
    for segment in days.iter().flat_map(|d| &d.segments) {
        if legend.iter().all(|item| item.id != segment.product_id) {
            legend.push(DailyUsageLegendItem {
                id: segment.product_id.clone(),
                display_name: segment.display_name.clone(),
                color_token: segment.color_token,
            });
        }
    }
    legend.sort_by_key(|item| {
        ProductCatalog::DISPLAY_ORDER
            .iter()
            .position(|id| *id == item.id)
            .unwrap_or(99)
    });
    let has_daily_data = days.iter().any(|d| !d.segments.is_empty());
    DailyUsageWeek {
        week_start,
        week_end,
        days,
        legend_products: legend,
        has_daily_data,
        is_estimated,
    }
}

fn to_segment(product: &ProductUsage) -> DailyUsageSegment {
    DailyUsageSegment {
        product_id: product.id.clone(),
        display_name: product.display_name.clone(),
        percent_of_weekly: product.percent_of_pool,
        color_token: product.color_token,
    }
}

fn other_segment(total: f64) -> DailyUsageSegment {
    DailyUsageSegment {
        product_id: "other".to_owned(),
        display_name: "Other".to_owned(),
        percent_of_weekly: total,
        color_token: ProductColor::Other,
    }
}

fn sorted_visible(products: &[ProductUsage]) -> Vec<ProductUsage> {
    let mut visible: Vec<ProductUsage> = products
        .iter()
        .filter(|p| p.percent_of_pool > NOISE_FLOOR)
        .cloned()
        .collect();
    ProductCatalog::sort_for_display(&mut visible);
    visible
}

/// Absolute product slices above the noise floor; one `other` segment for
/// `fallback_total` when none remain.
fn segments_from_products(
    products: &[ProductUsage],
    fallback_total: Option<f64>,
) -> Vec<DailyUsageSegment> {
    let visible = sorted_visible(products);
    if !visible.is_empty() {
        return visible.iter().map(to_segment).collect();
    }
    match fallback_total {
        Some(total) if total > NOISE_FLOOR => vec![other_segment(total)],
        _ => Vec::new(),
    }
}

/// Day-over-day growth: per-product increases when available, else the total
/// delta weighted by the product mix.
fn growth_segments(
    previous_used: f64,
    day_used: f64,
    previous_products: &[ProductUsage],
    products: &[ProductUsage],
) -> Vec<DailyUsageSegment> {
    let by_product = product_delta_segments(previous_products, products);
    let product_sum: f64 = by_product.iter().map(|s| s.percent_of_weekly).sum();
    let total_delta = (day_used - previous_used).max(0.0);
    if !by_product.is_empty() && product_sum > NOISE_FLOOR {
        by_product
    } else if total_delta > NOISE_FLOOR {
        distribute(total_delta, products)
    } else {
        Vec::new()
    }
}

fn product_delta_segments(
    previous: &[ProductUsage],
    current: &[ProductUsage],
) -> Vec<DailyUsageSegment> {
    let previous_by_id: BTreeMap<String, f64> = previous
        .iter()
        .map(|p| (p.id.to_lowercase(), p.percent_of_pool))
        .collect();
    let mut deltas: Vec<ProductUsage> = current
        .iter()
        .filter_map(|p| {
            let before = previous_by_id.get(&p.id.to_lowercase()).copied().unwrap_or(0.0);
            let delta = (p.percent_of_pool - before).max(0.0);
            (delta > NOISE_FLOOR).then(|| {
                ProductUsage::new(&p.id, p.display_name.clone(), delta).with_color(p.color_token)
            })
        })
        .collect();
    ProductCatalog::sort_for_display(&mut deltas);
    deltas.iter().map(to_segment).collect()
}

/// Proportional split of `total` over the current mix; one `other` segment
/// when no products exist.
fn distribute(total: f64, products: &[ProductUsage]) -> Vec<DailyUsageSegment> {
    if total <= NOISE_FLOOR {
        return Vec::new();
    }
    let visible = sorted_visible(products);
    let sum: f64 = visible.iter().map(|p| p.percent_of_pool).sum();
    if visible.is_empty() || sum < NOISE_FLOOR {
        return vec![other_segment(total)];
    }
    visible
        .iter()
        .filter_map(|p| {
            let share = total * (p.percent_of_pool / sum);
            (share > NOISE_FLOOR).then(|| DailyUsageSegment {
                percent_of_weekly: share,
                ..to_segment(p)
            })
        })
        .collect()
}

/// Synthetic Thursday-to-Wednesday week with sample product bars, for UI
/// previews.
pub fn preview(cal: &dyn DayCalendar) -> DailyUsageWeek {
    let reset = DateTime::parse_from_rfc3339("2026-07-16T20:25:00Z")
        .map_or_else(|_| DateTime::<Utc>::UNIX_EPOCH, |d| d.with_timezone(&Utc));
    let now = reset - chrono::TimeDelta::days(2);
    let Some((week_start, week_end)) = billing_period_week_bounds(Some(reset), 0, cal, now) else {
        return finalize(now, now, Vec::new(), true);
    };
    const PATTERN: [(f64, f64, f64); 7] = [
        (5.0, 1.0, 0.0),
        (7.0, 2.0, 1.0),
        (2.0, 1.0, 0.0),
        (0.0, 0.0, 0.0),
        (18.0, 5.0, 1.0),
        (4.0, 1.0, 0.0),
        (3.0, 0.0, 0.0),
    ];
    let days = PATTERN
        .iter()
        .zip(0i64..)
        .map(|((build, api, chat), offset)| {
            let segments = [("build", *build), ("api", *api), ("chat", *chat)]
                .into_iter()
                .filter(|(_, pct)| *pct > 0.0)
                .map(|(id, pct)| {
                    to_segment(&ProductUsage::new(id, ProductCatalog::display_name(id), pct))
                })
                .collect();
            let bar = Bar {
                segments,
                is_after_reset: false,
            };
            let day_start = cal.add_days(week_start, offset);
            DailyUsageDay {
                is_today: false,
                ..make_day(day_start, bar, cal, now)
            }
        })
        .collect();
    finalize(week_start, week_end, days, false)
}
