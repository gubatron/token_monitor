//! Event aggregation, the event ledger and the single-entry cache (spec 02
//! section 3.2.2).
//!
//! Everything here is pure over `&[CursorUsageEvent]` plus an injected
//! [`Clock`] for the local calendar, so it is deterministic in tests.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Days, NaiveDate, Timelike, Utc};
use tokenmon_core::Clock;
use tokenmon_core::usage::{Calendar, DailyBudget, DailyTokenUsage, TokenCounts, UsageSource};

use crate::events::CursorUsageEvent;
use crate::models::{CursorCostStats, CursorSnapshot};

/// Aggregates younger than this are served without any network.
pub const EVENTS_REFRESH_INTERVAL: Duration = Duration::from_mins(5);
/// Incremental fetches re-request this much history to catch late events.
pub const EVENTS_OVERLAP: Duration = Duration::from_mins(15);
/// After this long the whole window is paged again.
pub const EVENTS_FULL_REFRESH_INTERVAL: Duration = Duration::from_hours(1);
/// Events per page.
pub const EVENTS_PAGE_SIZE: usize = 500;
/// Page cap (20,000 events).
pub const EVENTS_MAX_PAGES: usize = 40;
/// The events window never reaches back more than this many days.
pub const EVENTS_WINDOW_CAP_DAYS: u64 = 31;
/// `QuotaNormalization.averageWeeksPerMonth`.
pub const AVERAGE_WEEKS_PER_MONTH: f64 = 365.2425 / 12.0 / 7.0;

/// Local midnight of `date` (the date's UTC midnight if the offset overflows).
fn midnight(cal: &Calendar<'_>, date: NaiveDate) -> DateTime<Utc> {
    cal.start_of_date(date)
        .unwrap_or_else(|| date.and_time(chrono::NaiveTime::MIN).and_utc())
}

/// Keeps a float sum finite: hostile input could otherwise overflow to infinity.
fn cap(value: f64) -> f64 {
    value.min(f64::MAX)
}

/// `eventsWindowStart`: the cycle start, but never before local midnight 31
/// days ago.
pub fn events_window_start(
    clock: &dyn Clock,
    cycle_start: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let cal = Calendar::local(clock);
    let back = cal.add_days(now, -i64::try_from(EVENTS_WINDOW_CAP_DAYS).unwrap_or(31));
    let cap = cal.start_of_day(back.unwrap_or(now));
    cycle_start.map_or(cap, |start| start.max(cap))
}

/// `aggregateCostStats`: cycle cost and tokens over events at or after
/// `cycle_start`.
pub fn aggregate_cost_stats(
    events: &[CursorUsageEvent],
    cycle_start: DateTime<Utc>,
) -> CursorCostStats {
    let init = (0.0, 0_i64, 0_i64, 0_i64);
    let (cents, total, input, output) =
        events
            .iter()
            .filter(|e| e.date >= cycle_start)
            .fold(init, |(c, t, i, o), e| {
                (
                    cap(c + e.charged_cents),
                    t.saturating_add(e.total_tokens),
                    i.saturating_add(e.input_tokens),
                    o.saturating_add(e.output_tokens),
                )
            });
    CursorCostStats {
        metered_cycle_usd: cents / 100.0,
        cycle_tokens: total,
        cycle_input_tokens: input,
        cycle_output_tokens: output,
    }
}

/// Hourly activity of one local day (Overview hourly chart input).
#[derive(Debug, Clone, PartialEq)]
pub struct CursorDayHourlyUsage {
    /// Local midnight of the day.
    pub day_start: DateTime<Utc>,
    /// Activity weight per local hour.
    pub hour_weights: [f64; 24],
    /// Quota percentage points per hour.
    pub quota_hour_weights: [f64; 24],
    /// Tokens per hour.
    pub hour_token_weights: [i64; 24],
}

impl CursorDayHourlyUsage {
    /// An all-zero day.
    pub fn empty(day_start: DateTime<Utc>) -> Self {
        Self {
            day_start,
            hour_weights: [0.0; 24],
            quota_hour_weights: [0.0; 24],
            hour_token_weights: [0; 24],
        }
    }

    /// Whether there is no activity at all.
    pub fn is_empty(&self) -> bool {
        self.quota_hour_weights.iter().all(|w| *w <= 0.0)
            && self.hour_weights.iter().all(|w| *w <= 0.0)
    }
}

/// The events of the local day that starts at `day_start`, with their hour.
fn hourly_events<'a>(
    clock: &'a dyn Clock,
    events: &'a [CursorUsageEvent],
    day_start: DateTime<Utc>,
) -> impl Iterator<Item = (usize, &'a CursorUsageEvent)> {
    let cal = Calendar::local(clock);
    let date = cal.date_of(day_start);
    let start = midnight(&cal, date);
    let end = date
        .checked_add_days(Days::new(1))
        .map_or(start + chrono::Duration::days(1), |d| midnight(&cal, d));
    events
        .iter()
        .filter(move |e| e.date >= start && e.date < end)
        .map(move |e| (cal.local_naive(e.date).hour() as usize, e))
}

/// `hourWeights`: [`CursorUsageEvent::weight`] per hour.
pub fn hour_weights(
    clock: &dyn Clock,
    events: &[CursorUsageEvent],
    day_start: DateTime<Utc>,
) -> [f64; 24] {
    let mut out = [0.0; 24];
    for (hour, event) in hourly_events(clock, events, day_start) {
        out[hour] = cap(out[hour] + event.weight());
    }
    out
}

/// `quotaHourWeights`: charged cents as percentage points of the plan quota
/// (monthly limit divided by the average weeks per month). All zeros without a
/// positive plan limit.
pub fn quota_hour_weights(
    clock: &dyn Clock,
    events: &[CursorUsageEvent],
    day_start: DateTime<Utc>,
    plan_limit_usd: Option<f64>,
) -> [f64; 24] {
    let mut out = [0.0; 24];
    let Some(limit) = plan_limit_usd.filter(|l| *l > 0.0) else {
        return out;
    };
    let limit_cents = limit * 100.0 / AVERAGE_WEEKS_PER_MONTH;
    for (hour, event) in hourly_events(clock, events, day_start) {
        if event.charged_cents > 0.0 {
            out[hour] = cap(out[hour] + event.charged_cents / limit_cents * 100.0);
        }
    }
    out
}

/// `tokenHourWeights`: total tokens per hour.
pub fn token_hour_weights(
    clock: &dyn Clock,
    events: &[CursorUsageEvent],
    day_start: DateTime<Utc>,
) -> [i64; 24] {
    let mut out = [0_i64; 24];
    for (hour, event) in hourly_events(clock, events, day_start) {
        out[hour] = out[hour].saturating_add(event.total_tokens);
    }
    out
}

/// Cursor's usage events come from its servers and cover the whole account,
/// so its daily tokens are never labelled as this PC's.
pub const CURSOR_SOURCE: UsageSource = UsageSource::Account;

/// `dailyTokens`: per local day tokens, model split and cost.
pub fn daily_tokens(
    clock: &dyn Clock,
    events: &[CursorUsageEvent],
) -> BTreeMap<NaiveDate, DailyTokenUsage> {
    let cal = Calendar::local(clock);
    let mut days: BTreeMap<NaiveDate, DailyTokenUsage> = BTreeMap::new();
    for event in events {
        let counts = TokenCounts {
            input: event.input_tokens,
            output: event.output_tokens,
            cache_read: event.cache_read_tokens,
            cache_write: event.cache_write_tokens,
        };
        days.entry(cal.date_of(event.date))
            .or_insert_with(|| DailyTokenUsage::new(CURSOR_SOURCE))
            .add(&event.model, counts, Some(event.charged_cents / 100.0));
    }
    // Hostile input could overflow a float sum to infinity.
    for day in days.values_mut() {
        day.cost_usd = day.cost_usd.map(cap);
    }
    days
}

/// `estimatedWeightByDay`: total tokens per local day, restricted to the
/// subscription month that contains `now`.
pub fn estimated_weight_by_day(
    clock: &dyn Clock,
    events: &[CursorUsageEvent],
    cycle_start: DateTime<Utc>,
    cycle_end: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> BTreeMap<NaiveDate, f64> {
    let cal = Calendar::local(clock);
    let bounds = DailyBudget::subscription_month(Some(cycle_start), cycle_end, Some(now), &cal);
    let lower = bounds.map_or(cycle_start, |b| b.start);
    let upper = bounds.map(|b| b.end).or(cycle_end);
    let mut by_day = BTreeMap::new();
    for event in events.iter().filter(|e| e.total_tokens > 0) {
        if event.date < lower || upper.is_some_and(|u| event.date >= u) {
            continue;
        }
        let slot = by_day.entry(cal.date_of(event.date)).or_insert(0.0);
        *slot = cap(*slot + event.total_tokens as f64);
    }
    by_day
}

/// Everything derived from one events fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorEventAggregates {
    /// When the events were fetched.
    pub fetched_at: DateTime<Utc>,
    /// The window these aggregates cover.
    pub window_start: DateTime<Utc>,
    /// Cycle cost and tokens.
    pub cost_stats: CursorCostStats,
    /// Hourly activity of today.
    pub hourly: CursorDayHourlyUsage,
    /// Tokens per day, for budget estimation.
    pub estimated_weight_by_day: BTreeMap<NaiveDate, f64>,
    /// Tokens per day, for the hover label.
    pub tokens_by_day: BTreeMap<NaiveDate, DailyTokenUsage>,
}

impl CursorEventAggregates {
    /// Whether these can be served again without network: same window, same
    /// local day, younger than five minutes and not from the future.
    pub fn is_fresh(
        &self,
        clock: &dyn Clock,
        window_start: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> bool {
        let age = now - self.fetched_at;
        self.window_start == window_start
            && Calendar::local(clock).date_of(self.fetched_at)
                == Calendar::local(clock).date_of(now)
            && age >= chrono::Duration::zero()
            && age < chrono_secs(EVENTS_REFRESH_INTERVAL)
    }
}

/// `aggregate(events, snap, windowStart, now)`.
pub fn aggregate_events(
    clock: &dyn Clock,
    events: &[CursorUsageEvent],
    snapshot: &CursorSnapshot,
    window_start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> CursorEventAggregates {
    let cycle_start = snapshot.billing_cycle_start.unwrap_or(window_start);
    let day_start = Calendar::local(clock).start_of_day(now);
    CursorEventAggregates {
        fetched_at: now,
        window_start,
        cost_stats: aggregate_cost_stats(events, cycle_start),
        hourly: CursorDayHourlyUsage {
            day_start,
            hour_weights: hour_weights(clock, events, day_start),
            quota_hour_weights: quota_hour_weights(
                clock,
                events,
                day_start,
                snapshot.plan_limit_usd,
            ),
            hour_token_weights: token_hour_weights(clock, events, day_start),
        },
        estimated_weight_by_day: estimated_weight_by_day(
            clock,
            events,
            cycle_start,
            snapshot.billing_cycle_end,
            now,
        ),
        tokens_by_day: daily_tokens(clock, events),
    }
}

fn chrono_secs(d: Duration) -> chrono::Duration {
    chrono::Duration::seconds(i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// All events fetched for one window, so later refreshes can extend it.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorEventLedger {
    /// Window the ledger covers.
    pub window_start: DateTime<Utc>,
    /// Events, any order.
    pub events: Vec<CursorUsageEvent>,
    /// Newest instant covered by a fetch.
    pub fetched_through: DateTime<Utc>,
    /// When the whole window was last paged.
    pub full_fetch_at: DateTime<Utc>,
}

impl CursorEventLedger {
    /// Whether an incremental fetch may extend this ledger: same window, the
    /// clock has not gone backwards and the last full fetch is under an hour
    /// old.
    pub fn can_extend(&self, window_start: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        self.window_start == window_start
            && now >= self.fetched_through
            && now - self.full_fetch_at < chrono_secs(EVENTS_FULL_REFRESH_INTERVAL)
    }

    /// Where an incremental fetch starts: the overlap before `fetched_through`,
    /// never before the window.
    pub fn incremental_from(&self) -> DateTime<Utc> {
        self.window_start
            .max(self.fetched_through - chrono_secs(EVENTS_OVERLAP))
    }

    /// The ledger after an incremental fetch: old events before `from` plus
    /// `fresh` (overlap events are replaced, not duplicated).
    #[must_use]
    pub fn extended(
        &self,
        from: DateTime<Utc>,
        fresh: Vec<CursorUsageEvent>,
        now: DateTime<Utc>,
    ) -> Self {
        let events = self
            .events
            .iter()
            .filter(|e| e.date < from)
            .cloned()
            .chain(fresh)
            .collect();
        Self {
            events,
            fetched_through: now,
            ..self.clone()
        }
    }
}

/// What the cache holds for one session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CursorCacheEntry {
    /// Last aggregates.
    pub aggregates: Option<CursorEventAggregates>,
    /// Last ledger.
    pub ledger: Option<CursorEventLedger>,
}

/// In-memory, single-entry cache keyed by the cookie header, so each session
/// only ever sees its own data. Never persisted. `Debug` redacts the key.
#[derive(Default)]
pub struct CursorEventCache {
    slot: Mutex<Option<(String, CursorCacheEntry)>>,
}

impl fmt::Debug for CursorEventCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorEventCache").finish_non_exhaustive()
    }
}

impl CursorEventCache {
    /// The entry stored for exactly this cookie header.
    pub fn get(&self, key: &str) -> Option<CursorCacheEntry> {
        let slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, e)| e.clone())
    }

    /// Replaces the (single) entry.
    pub fn store(&self, key: &str, entry: CursorCacheEntry) {
        *self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((key.to_owned(), entry));
    }

    /// Drops everything (sign-out, account reset).
    pub fn clear(&self) {
        *self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Whether anything is cached.
    pub fn is_empty(&self) -> bool {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
    }
}
