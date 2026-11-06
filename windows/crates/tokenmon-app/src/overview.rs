//! Overview data: the saved per-day hourly history, today's live series built
//! from the pollers' hourly data, and the midnight rollover (spec 01 section
//! 4).
//!
//! Portable: the pollers are reached only through the registry's
//! [`HourlySource`](crate::sources::HourlySource) closures, the clock is an
//! argument, and the history file lives in the data directory.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use tokenmon_core::ProviderId;
use tokenmon_core::overview::{
    OverviewDaySeries, OverviewHistory, OverviewSeries, ProviderGate, TodayInputs,
    build_today_series, day_for_offset,
};
use tokenmon_core::usage::Calendar;
use tokenmon_ui::panels::overview::{
    HourActivityInput, HourSeries, OverviewDailyInput, midnight_schedule,
};

use crate::registry::ProviderRegistry;
use crate::sessions::Sessions;
use crate::sources::TodayPart;
use tokenmon_core::settings::Settings;

/// The history file inside the data directory (spec 01 section 4.3).
pub const HISTORY_FILE: &str = "activity_overview_hourly_history.dat";

const HOURS: usize = 24;

/// The UI series for a core series.
pub fn hour_series(series: OverviewSeries) -> HourSeries {
    match series {
        OverviewSeries::Grok => HourSeries::Grok,
        OverviewSeries::Grokbot => HourSeries::Grokbot,
        OverviewSeries::Cursor => HourSeries::Cursor,
        OverviewSeries::Claude => HourSeries::Claude,
        OverviewSeries::OpenCodeGo => HourSeries::OpenCodeGo,
        OverviewSeries::OpenCodeZen => HourSeries::OpenCodeZen,
    }
}

/// The 24 hour columns of `series`, each listing the series active in that
/// hour in stack order.
pub fn hour_columns(series: &OverviewDaySeries) -> Vec<Vec<HourActivityInput>> {
    (0..HOURS)
        .map(|hour| {
            OverviewSeries::STACK_ORDER
                .into_iter()
                .filter_map(|s| {
                    let weight = series.weights.get(&s)?.get(hour).copied()?;
                    (weight.is_finite() && weight > 0.0).then(|| HourActivityInput {
                        series: hour_series(s),
                        quota_percent: weight,
                        tokens: series
                            .tokens
                            .get(&s)
                            .and_then(|t| t.get(hour))
                            .map(|t| u64::try_from(*t).unwrap_or(0)),
                    })
                })
                .collect()
        })
        .collect()
}

/// Whether `provider` contributes to the Overview right now: enabled, signed
/// in and not flagged for re-sign-in (OpenCode needs only to be enabled;
/// Grokbot passes Cursor's session gate).
pub fn gate(provider: ProviderId, settings: &Settings, sessions: &Sessions) -> ProviderGate {
    let state = sessions.auth_state(provider);
    ProviderGate {
        enabled: settings.is_provider_enabled(provider),
        signed_in: state.is_signed_in,
        needs_sign_in: state.needs_sign_in,
    }
}

fn contributes(provider: ProviderId, settings: &Settings, sessions: &Sessions) -> bool {
    let g = gate(provider, settings, sessions);
    if provider == ProviderId::OpenCode {
        g.contributes_without_sign_in()
    } else {
        g.contributes()
    }
}

/// Today's series from every contributing provider's hourly data.
pub fn today_series(
    registry: &ProviderRegistry,
    settings: &Settings,
    sessions: &Sessions,
    calendar: &Calendar<'_>,
    today: NaiveDate,
) -> OverviewDaySeries {
    let inputs = ProviderId::USAGE_PROVIDERS
        .into_iter()
        .filter(|p| contributes(*p, settings, sessions))
        .filter_map(|p| registry.hourly(p).and_then(|source| source(calendar)))
        .fold(TodayInputs::default(), |mut acc, part| {
            match part {
                TodayPart::Grok(d) => acc.grok = Some(d),
                TodayPart::Claude(d) => acc.claude = Some(d),
                TodayPart::Grokbot(d) => acc.grokbot = Some(d),
                TodayPart::Cursor(d) => acc.cursor = Some(d),
                TodayPart::OpenCode(d) => acc.open_code = Some(d),
            }
            acc
        });
    build_today_series(&inputs, today)
}

/// The first local midnight after `now`: when the Overview must roll to a new
/// day.
pub fn next_midnight(now: DateTime<Utc>, tz: FixedOffset) -> DateTime<Utc> {
    midnight_schedule(now, tz)
        .into_iter()
        .find(|m| *m > now)
        .unwrap_or_else(|| now + Duration::days(1))
}

/// The persisted Overview history.
pub struct OverviewStore {
    path: PathBuf,
    history: Mutex<OverviewHistory>,
}

impl OverviewStore {
    /// Loads the history from `directory` (empty when missing or damaged).
    pub fn open(directory: &Path) -> Self {
        let path = directory.join(HISTORY_FILE);
        Self {
            history: Mutex::new(OverviewHistory::load(&path)),
            path,
        }
    }

    fn history(&self) -> std::sync::MutexGuard<'_, OverviewHistory> {
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Merges `series` into `date` and saves when anything changed. Returns
    /// whether it did.
    pub fn record(&self, series: &OverviewDaySeries, date: NaiveDate) -> bool {
        let mut history = self.history();
        let changed = history.record(series, date);
        if changed && let Err(e) = history.save(&self.path) {
            tracing::warn!(error = %e, "overview history not saved");
        }
        changed
    }

    /// The saved series for `date`.
    pub fn series(&self, date: NaiveDate) -> Option<OverviewDaySeries> {
        self.history().series(date).cloned()
    }

    /// The earliest saved day.
    pub fn earliest_day(&self) -> Option<NaiveDate> {
        self.history().earliest_day()
    }

    /// The chart input for `day_offset` (`0` today, negative earlier): today
    /// is recorded first so the chart includes the newest hour.
    pub fn daily_input(
        &self,
        day_offset: i32,
        live: &OverviewDaySeries,
        today: NaiveDate,
    ) -> OverviewDailyInput {
        self.record(live, today);
        let date = day_for_offset(today, day_offset);
        let hours = self
            .series(date)
            .map_or_else(|| vec![Vec::new(); HOURS], |s| hour_columns(&s));
        OverviewDailyInput {
            day_offset,
            hours,
            earliest_day: self.earliest_day(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, n).unwrap()
    }

    /// Only series with weight in an hour appear, in stack order, with
    /// tokens where the series reports them.
    #[test]
    fn test_hour_columns_list_active_series_in_stack_order() {
        let mut grok = vec![0.0; 24];
        grok[9] = 2.0;
        let mut cursor = vec![0.0; 24];
        cursor[9] = 1.0;
        let mut tokens = vec![0; 24];
        tokens[9] = 500;
        let series = OverviewDaySeries::default()
            .with_weights(OverviewSeries::Cursor, cursor)
            .with_weights(OverviewSeries::Grok, grok)
            .with_tokens(OverviewSeries::Cursor, tokens);
        let columns = hour_columns(&series);
        assert_eq!(columns.len(), 24);
        assert!(columns[8].is_empty());
        let nine = &columns[9];
        assert_eq!(nine.len(), 2);
        assert_eq!(nine[0].series, HourSeries::Grok);
        assert_eq!(nine[0].tokens, None);
        assert_eq!(nine[1].series, HourSeries::Cursor);
        assert_eq!(nine[1].tokens, Some(500));
    }

    /// Recording persists, and a reopened store reads the day back.
    #[test]
    fn test_store_round_trips_through_the_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = vec![0.0; 24];
        w[3] = 1.5;
        let series = OverviewDaySeries::default().with_weights(OverviewSeries::Claude, w);
        let store = OverviewStore::open(dir.path());
        assert!(store.record(&series, day(10)));
        assert!(!store.record(&series, day(10)), "no change, no save");
        let reopened = OverviewStore::open(dir.path());
        assert_eq!(reopened.earliest_day(), Some(day(10)));
        let input = reopened.daily_input(0, &OverviewDaySeries::default(), day(10));
        assert_eq!(input.hours[3].len(), 1);
    }

    /// An earlier offset reads the earlier saved day; a missing day is empty.
    #[test]
    fn test_daily_input_offset_reads_earlier_days() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = vec![0.0; 24];
        w[1] = 1.0;
        let series = OverviewDaySeries::default().with_weights(OverviewSeries::Grok, w);
        let store = OverviewStore::open(dir.path());
        store.record(&series, day(9));
        let yesterday = store.daily_input(-1, &OverviewDaySeries::default(), day(10));
        assert_eq!(yesterday.hours[1].len(), 1);
        let none = store.daily_input(-2, &OverviewDaySeries::default(), day(10));
        assert!(none.hours.iter().all(Vec::is_empty));
        assert_eq!(none.earliest_day, Some(day(9)));
    }

    /// The next rollover is the following local midnight, DST-safe by using
    /// the UI schedule.
    #[test]
    fn test_next_midnight_is_the_following_local_midnight() {
        use chrono::TimeZone;
        let tz = FixedOffset::west_opt(7 * 3600).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 7, 10, 3, 0, 0).unwrap();
        // 03:00 UTC is 20:00 the previous local day; local midnight is 07:00 UTC.
        assert_eq!(
            next_midnight(now, tz),
            Utc.with_ymd_and_hms(2026, 7, 10, 7, 0, 0).unwrap()
        );
    }
}
