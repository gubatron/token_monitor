//! Monthly Daily Budget packaging (spec 03 section 3.8, steps 1 to 7).
//!
//! Turns the local scan (and the console's reset instant when signed in) into
//! the seven Monday-to-Sunday bars of the subscription month, expressed as a
//! percent of the monthly limit. The calendar maths is core's
//! [`DailyBudget`]; the percent maths is [`crate::stats`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use tokenmon_core::usage::budget::SpentByDay;
use tokenmon_core::usage::tokens::attaching_tokens_map;
use tokenmon_core::usage::{
    Calendar, DailyBudget, DailyBudgetDay, DailyTokenUsage, DailyTokenUsageTally, DayKey,
    MonthBounds, TokenCounts, UsageSource, WeekOffset,
};

use crate::bounds::{Bounds, monthly_bounds};
use crate::db::{DbError, MessageRow, Scan, scan_start};
use crate::model_name::display_name;
use crate::pricing::is_go_provider;
use crate::stats::{scaled_spends_percent, spread_used_percent};

/// Where OpenCode's token counts come from: `opencode.db` on this PC, not
/// the account's usage.
pub const LOCAL_SOURCE: UsageSource = UsageSource::ThisComputer;

/// How far before the period start spends are read, as dimmed history bars.
pub const PRIOR_WEEK_HISTORY_DAYS: i64 = 28;

/// Loads the scan of rows since the given instant (the live one is cached).
pub type ScanFn = dyn Fn(DateTime<Utc>) -> Result<Arc<Scan>, DbError> + Send + Sync;

/// Inputs of [`month_daily_budget_days`].
#[derive(Debug, Clone, Copy)]
pub struct MonthBudgetRequest<'a> {
    /// Monthly limit in USD; not positive gives empty percent bars.
    pub limit_usd: f64,
    /// The Monthly bar's used percent; the bars are scaled to sum to it.
    pub used_percent: f64,
    /// The console's monthly reset instant, which wins over local anchors.
    pub period_resets_at: Option<DateTime<Utc>>,
    /// How many weeks back to show.
    pub week_offset: WeekOffset,
    /// Reference instant.
    pub now: DateTime<Utc>,
    /// Injected USD spends per local day start (tests); `None` reads the scan.
    pub spent_by_day: Option<&'a SpentByDay>,
}

/// The bars plus everything needed to rebuild other weeks without a new scan
/// (`OpenCodeMonthBudget`).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCodeMonthBudget {
    /// Bars of the requested week.
    pub days: Vec<DailyBudgetDay>,
    /// Start of the first day of the billing period.
    pub period_start: DateTime<Utc>,
    /// Anchored spend percent per local day within the period.
    pub spent_percent_by_day: SpentByDay,
    /// Spend percent per day before the period (dimmed bars).
    pub history_percent_by_day: SpentByDay,
    /// Period start when known locally.
    pub known_start: Option<DateTime<Utc>>,
    /// Period end when known.
    pub resets_at: Option<DateTime<Utc>>,
    /// Instant the bars were built against.
    pub reference_now: DateTime<Utc>,
}

impl OpenCodeMonthBudget {
    /// Bars for the week `week_offset` steps back, from the stored percents
    /// (no new database read). [`WeekOffset::CURRENT`] gives [`Self::days`].
    #[must_use]
    pub fn days_at(
        &self,
        week_offset: WeekOffset,
        cal: &Calendar<'_>,
    ) -> Option<Vec<DailyBudgetDay>> {
        if week_offset == WeekOffset::CURRENT {
            return Some(self.days.clone());
        }
        DailyBudget::build_subscription_month_last_7_days(
            100.0,
            &self.spent_percent_by_day,
            self.known_start,
            self.resets_at,
            &self.history_percent_by_day,
            week_offset,
            self.reference_now,
            cal,
        )
        .map(|(days, _)| days)
    }
}

/// USD spent per local day start by the given rows; non-positive billable
/// costs are skipped.
pub fn daily_spends<'a>(
    rows: impl Iterator<Item = &'a MessageRow>,
    cal: &Calendar<'_>,
) -> SpentByDay {
    let mut out = SpentByDay::new();
    for row in rows {
        let cost = row.billable().cost;
        if cost > 0.0 {
            *out.entry(cal.start_of_day(row.date)).or_insert(0.0) += cost;
        }
    }
    out
}

fn go_rows_between(
    scan: &Scan,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> impl Iterator<Item = &MessageRow> {
    scan.rows
        .iter()
        .filter(move |r| is_go_provider(&r.provider_id) && r.date >= from && r.date < to)
}

/// Go spends in the subscription month containing `now`
/// (`fetchMonthDailySpends`); any read error gives an empty map.
pub fn fetch_month_daily_spends(
    load: &ScanFn,
    now: DateTime<Utc>,
    cal: &Calendar<'_>,
) -> SpentByDay {
    let Ok(scan) = load(scan_start(now)) else {
        return SpentByDay::new();
    };
    let Bounds { start, end } = monthly_bounds(now, scan.subscribed_at.unwrap_or(now));
    daily_spends(go_rows_between(&scan, start, end), cal)
}

/// `(period, history)` spends for `[start, end)` with 28 days of history
/// before it; any read error gives two empty maps.
pub fn spend_history(
    load: &ScanFn,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    cal: &Calendar<'_>,
) -> (SpentByDay, SpentByDay) {
    let start_day = cal.start_of_day(start);
    let from = cal.add_days(start_day, -PRIOR_WEEK_HISTORY_DAYS);
    let scan = from.and_then(|from| load(from).ok().map(|s| (from, s)));
    let Some((from, scan)) = scan else {
        return (SpentByDay::new(), SpentByDay::new());
    };
    let all = daily_spends(go_rows_between(&scan, from, end), cal);
    all.into_iter().partition(|(day, _)| *day >= start_day)
}

fn percent_of(limit: f64, usd: &SpentByDay) -> SpentByDay {
    if limit <= 0.0 {
        return SpentByDay::new();
    }
    usd.iter().map(|(d, v)| (*d, v / limit * 100.0)).collect()
}

fn to_dates(by_day: &SpentByDay, cal: &Calendar<'_>) -> BTreeMap<NaiveDate, f64> {
    let mut out = BTreeMap::new();
    for (day, v) in by_day {
        *out.entry(cal.date_of(*day)).or_insert(0.0) += v;
    }
    out
}

fn from_dates(by_date: &BTreeMap<NaiveDate, f64>, cal: &Calendar<'_>) -> SpentByDay {
    by_date
        .iter()
        .filter_map(|(d, v)| cal.start_of_date(*d).map(|s| (s, *v)))
        .collect()
}

/// Steps 3 and 4: percent of the limit, scaled to the used percent, or the
/// used percent spread over the elapsed days when there are no local spends.
fn anchored_percent(
    usd: &SpentByDay,
    req: &MonthBudgetRequest<'_>,
    console: Option<MonthBounds>,
    cal: &Calendar<'_>,
) -> SpentByDay {
    let percent = to_dates(&percent_of(req.limit_usd, usd), cal);
    let scaled = scaled_spends_percent(&percent, req.used_percent);
    if scaled.is_empty()
        && req.used_percent > 0.0
        && let Some(bounds) = console
    {
        let spread = spread_used_percent(
            cal.date_of(bounds.start),
            cal.date_of(req.now),
            req.used_percent,
        );
        return from_dates(&spread, cal);
    }
    from_dates(&scaled, cal)
}

/// Step 5: hands the anchored percents to core's bar builder.
struct Packager<'a> {
    req: &'a MonthBudgetRequest<'a>,
    anchored: SpentByDay,
    cal: &'a Calendar<'a>,
}

impl Packager<'_> {
    fn package(
        &self,
        known_start: Option<DateTime<Utc>>,
        resets_at: Option<DateTime<Utc>>,
        history_usd: &SpentByDay,
    ) -> Option<OpenCodeMonthBudget> {
        let history = percent_of(self.req.limit_usd, history_usd);
        let (days, period_start) = DailyBudget::build_subscription_month_last_7_days(
            100.0,
            &self.anchored,
            known_start,
            resets_at,
            &history,
            self.req.week_offset,
            self.req.now,
            self.cal,
        )?;
        Some(OpenCodeMonthBudget {
            days,
            period_start,
            spent_percent_by_day: self.anchored.clone(),
            history_percent_by_day: history,
            known_start,
            resets_at,
            reference_now: self.req.now,
        })
    }
}

/// The monthly Daily Budget (`monthDailyBudgetDays`): seven bars of the
/// subscription month as a percent of the monthly limit, anchored so the
/// current spends sum to the Monthly bar's used percent. `None` when neither
/// a console reset nor a local Go session fixes the period.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use chrono::{TimeZone, Utc};
/// use tokenmon_core::usage::{Calendar, WeekOffset};
/// use tokenmon_provider_opencode::budget::{MonthBudgetRequest, month_daily_budget_days};
/// use tokenmon_provider_opencode::db::Scan;
///
/// let now = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
/// let req = MonthBudgetRequest {
///     limit_usd: 60.0,
///     used_percent: 12.0,
///     period_resets_at: Some(Utc.with_ymd_and_hms(2026, 9, 5, 0, 0, 0).unwrap()),
///     week_offset: WeekOffset::CURRENT,
///     now,
///     spent_by_day: None,
/// };
/// let load = |_| Ok(Arc::new(Scan::default()));
/// let budget = month_daily_budget_days(&req, &load, &Calendar::utc()).unwrap();
/// let total: f64 = budget.days.iter().map(|d| d.spent_usd).sum();
/// assert!((total - 12.0).abs() < 1e-9);
/// ```
pub fn month_daily_budget_days(
    req: &MonthBudgetRequest<'_>,
    load: &ScanFn,
    cal: &Calendar<'_>,
) -> Option<OpenCodeMonthBudget> {
    let console = DailyBudget::subscription_month(None, req.period_resets_at, Some(req.now), cal);
    let (usd, history) = match (req.spent_by_day, console) {
        (Some(injected), _) => (injected.clone(), SpentByDay::new()),
        (None, Some(b)) => spend_history(load, b.start, b.end, cal),
        (None, None) => (
            fetch_month_daily_spends(load, req.now, cal),
            SpentByDay::new(),
        ),
    };
    let packager = Packager {
        req,
        anchored: anchored_percent(&usd, req, console, cal),
        cal,
    };
    if let Some(resets) = req.period_resets_at {
        return packager.package(None, Some(resets), &history);
    }
    let subscribed_at = load(scan_start(req.now)).ok()?.subscribed_at?;
    let month = monthly_bounds(req.now, subscribed_at);
    let history = if req.spent_by_day.is_none() && history.is_empty() {
        spend_history(load, month.start, month.end, cal).1
    } else {
        history
    };
    packager.package(Some(month.start), Some(month.end), &history)
}

/// Tokens per local day for Go rows since `since` (the bars' hover label).
/// Unpriced rows carry no cost. A read error gives an empty map.
pub fn daily_tokens(
    load: &ScanFn,
    since: DateTime<Utc>,
    cal: &Calendar<'_>,
) -> HashMap<DayKey, DailyTokenUsage> {
    let Ok(scan) = load(since) else {
        return HashMap::new();
    };
    let mut tally = DailyTokenUsageTally::new(*cal, LOCAL_SOURCE);
    for row in scan.rows.iter().filter(|r| is_go_provider(&r.provider_id)) {
        let billable = row.billable();
        tally.add_at(
            row.date,
            &display_name(&row.model_id),
            TokenCounts {
                input: row.tokens.input,
                output: row.tokens.output,
                cache_read: row.tokens.cache_read,
                cache_write: row.tokens.cache_write,
            },
            (!billable.is_unpriced).then_some(billable.cost),
        );
    }
    tally.into_days().into_iter().collect()
}

/// The bars with `tokens` attached to their local days.
#[must_use]
pub fn with_tokens<S: std::hash::BuildHasher>(
    days: Vec<DailyBudgetDay>,
    tokens: &HashMap<DayKey, DailyTokenUsage, S>,
    cal: &Calendar<'_>,
) -> Vec<DailyBudgetDay> {
    attaching_tokens_map(days, tokens, cal)
}
