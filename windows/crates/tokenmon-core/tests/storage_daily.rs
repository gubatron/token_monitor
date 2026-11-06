//! Ports of `DailyQuotaDeltaStoreTests` (spec 01 section 1.3.5 and 7).

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use common::{at, offset_clock, utc_clock};
use tokenmon_core::storage::{
    DailyQuotaDeltaStore, InMemoryStringStore, QuotaWindow, QuotaWindowTransition as T, TextStore,
    to_apple_seconds,
};

const KEY: &str = "claude_daily_usage";
const TODAY: &str = "2026-10-10";

fn new_store(backing: &Arc<InMemoryStringStore>) -> DailyQuotaDeltaStore {
    let store: Arc<dyn TextStore> = backing.clone();
    DailyQuotaDeltaStore::new(store, KEY, utc_clock(at(0, 12)))
}

fn today_total(store: &DailyQuotaDeltaStore) -> f64 {
    store.day_totals().get(TODAY).copied().unwrap_or(0.0)
}

fn window(start_h: Option<i64>, reset_h: i64) -> QuotaWindow {
    QuotaWindow {
        start: start_h.map(|h| at(0, 0) + Duration::hours(h)),
        resets_at: Some(at(0, 0) + Duration::hours(reset_h)),
    }
}

/// Growth between two samples is credited to the sampled day.
#[test]
fn test_growth_is_attributed_to_the_sampled_day() {
    let mut s = new_store(&Arc::default());
    s.record(10.0, at(0, 9), None, 7);
    s.record(25.0, at(0, 10), None, 7);
    assert_eq!(today_total(&s), 15.0);
}

/// A 5+ point drop is a reset: the post-reset value is credited.
#[test]
fn test_window_reset_attributes_post_reset_value_to_the_sampled_day() {
    let mut s = new_store(&Arc::default());
    for v in [40.0, 60.0, 25.0] {
        s.record(v, at(0, 9), None, 7);
    }
    assert_eq!(today_total(&s), 20.0 + 25.0);
}

/// Growth continues from the post-reset baseline.
#[test]
fn test_growth_continues_after_window_reset() {
    let mut s = new_store(&Arc::default());
    for v in [60.0, 25.0, 30.0] {
        s.record(v, at(0, 9), None, 7);
    }
    assert_eq!(today_total(&s), 30.0);
}

/// Deltas under 0.05 are noise.
#[test]
fn test_tiny_delta_is_ignored_as_noise() {
    let mut s = new_store(&Arc::default());
    s.record(10.0, at(0, 9), None, 7);
    s.record(10.01, at(0, 9), None, 7);
    assert_eq!(today_total(&s), 0.0);
}

/// A small downward tick is noise, not a reset.
#[test]
fn test_small_downward_noise_is_not_credited_as_reset() {
    let mut s = new_store(&Arc::default());
    s.record(50.0, at(0, 9), None, 7);
    s.record(49.9, at(0, 9), None, 7);
    assert_eq!(today_total(&s), 0.0);
}

/// Days older than 40 are pruned.
#[test]
fn test_old_days_are_pruned_to_retention_window() {
    let mut s = new_store(&Arc::default());
    s.record(0.0, at(-41, 9), None, 7);
    s.record(10.0, at(-41, 9), None, 7);
    assert_eq!(s.day_totals().len(), 1);
    s.record(21.0, at(0, 9), None, 7);
    assert_eq!(s.day_totals().len(), 1);
    assert_eq!(today_total(&s), 11.0);
}

/// `begin_new_window` empties the days; the next sample is credited whole.
#[test]
fn test_begin_new_window_clears_days_and_next_sample_is_credited_as_reset() {
    let mut s = new_store(&Arc::default());
    s.record(10.0, at(0, 9), None, 7);
    s.record(30.0, at(0, 9), None, 7);
    s.begin_new_window(Some(at(0, 9)), None);
    assert!(s.day_totals().is_empty());
    s.record(25.0, at(0, 10), None, 7);
    assert_eq!(today_total(&s), 25.0);
}

/// A rising first sample after `begin_new_window` is credited whole.
#[test]
fn test_begin_new_window_credits_rising_first_sample() {
    let mut s = new_store(&Arc::default());
    s.record(20.0, at(0, 9), None, 7);
    s.begin_new_window(Some(at(0, 9)), None);
    s.record(30.0, at(0, 10), None, 7);
    assert_eq!(today_total(&s), 30.0);
}

/// Days and the baseline survive a reload.
#[test]
fn test_reload_restores_persisted_days_and_baseline() {
    let backing = Arc::new(InMemoryStringStore::default());
    {
        let mut s = new_store(&backing);
        s.record(10.0, at(0, 9), None, 7);
        s.record(22.0, at(0, 9), None, 7);
    }
    let mut s = new_store(&backing);
    s.record(27.0, at(0, 10), None, 7);
    assert_eq!(today_total(&s), 17.0);
}

/// Early reset keeps prior days and credits the first post-reset sample.
#[test]
fn test_early_reset_preserves_prior_days_and_credits_first_post_reset_sample() {
    let mut s = new_store(&Arc::default());
    let old = window(Some(-3 * 24 + 8), 4 * 24 + 8);
    s.record(0.0, at(-3, 8), Some(old), 7);
    s.record(20.0, at(-2, 12), Some(old), 7);
    s.record(40.0, at(-1, 12), Some(old), 7);
    let new = window(Some(8), 7 * 24 + 8);
    assert_eq!(s.record(4.0, at(0, 9), Some(new), 7), T::EarlyReset);
    assert_eq!(s.day_totals()["2026-10-08"], 20.0);
    assert_eq!(s.day_totals()["2026-10-09"], 20.0);
    assert_eq!(today_total(&s), 4.0);
    assert_eq!(s.window_start(), Some(at(0, 8)));
    assert_eq!(s.interrupted_window_start(), Some(at(-3, 8)));
}

/// The first early-reset sample is credited whole and becomes the baseline.
#[test]
fn test_early_reset_baseline_credits_first_sample_whole() {
    let mut s = new_store(&Arc::default());
    let old = window(Some(-24), 4 * 24);
    s.record(0.0, at(0, 9), Some(old), 7);
    s.record(40.0, at(0, 9), Some(old), 7);
    let new = window(Some(10), 7 * 24 + 10);
    s.record(8.0, at(0, 11), Some(new), 7);
    s.record(11.0, at(0, 12), Some(new), 7);
    assert_eq!(today_total(&s), 11.0);
    assert_eq!(s.last_used_percent(), Some(11.0));
}

/// An early reset drops only the boundary day, not earlier ones.
#[test]
fn test_early_reset_drops_only_the_boundary_day() {
    let mut s = new_store(&Arc::default());
    let old = window(Some(-5 * 24), 3 * 24);
    s.record(0.0, at(-1, 9), Some(old), 7);
    s.record(20.0, at(-1, 9), Some(old), 7);
    s.record(28.0, at(0, 9), Some(old), 7);
    assert_eq!(today_total(&s), 8.0);
    let new = window(Some(10), 7 * 24 + 10);
    assert_eq!(s.record(2.0, at(0, 11), Some(new), 7), T::EarlyReset);
    assert_eq!(today_total(&s), 2.0);
    assert_eq!(s.day_totals()["2026-10-09"], 20.0);
}

/// A natural rollover starts a fresh window, keeps earlier days, and is not an interruption.
#[test]
fn test_normal_rollover_still_starts_fresh_window_and_keeps_earlier_days() {
    let mut s = new_store(&Arc::default());
    let old = window(Some(-6 * 24 + 8), 8);
    s.record(0.0, at(-1, 12), Some(old), 7);
    s.record(40.0, at(-1, 12), Some(old), 7);
    let new = window(None, 7 * 24 + 8);
    assert_eq!(s.record(3.0, at(0, 9), Some(new), 7), T::Rollover);
    assert_eq!(s.day_totals()["2026-10-09"], 40.0);
    assert_eq!(today_total(&s), 3.0);
    assert_eq!(s.window_start(), Some(at(0, 8)));
    assert_eq!(s.interrupted_window_start(), None);
}

/// A usage drop inside an unchanged window is a rebase, not an early reset.
#[test]
fn test_drop_with_unchanged_window_is_a_rebase_not_an_early_reset() {
    let mut s = new_store(&Arc::default());
    let w = window(Some(-24), 4 * 24);
    s.record(0.0, at(0, 9), Some(w), 7);
    s.record(50.0, at(0, 9), Some(w), 7);
    assert_eq!(s.record(10.0, at(0, 10), Some(w), 7), T::None);
    assert_eq!(s.interrupted_window_start(), None);
}

/// A moved window without a usage drop is not a reset.
#[test]
fn test_moved_window_without_usage_drop_is_not_a_reset() {
    let mut s = new_store(&Arc::default());
    s.record(10.0, at(0, 9), Some(window(Some(-24), 4 * 24)), 7);
    let moved = window(Some(9), 7 * 24 + 9);
    assert_eq!(s.record(12.0, at(0, 10), Some(moved), 7), T::None);
    assert_eq!(today_total(&s), 2.0);
}

/// With only `resets_at`, the start is derived from the reset instant.
#[test]
fn test_early_reset_without_period_start_derives_start_from_reset_instant() {
    let mut s = new_store(&Arc::default());
    let old = window(None, 4 * 24 + 8);
    s.record(0.0, at(-1, 12), Some(old), 7);
    s.record(25.0, at(-1, 12), Some(old), 7);
    let new = window(None, 7 * 24 + 8);
    assert_eq!(s.record(2.0, at(0, 9), Some(new), 7), T::EarlyReset);
    assert_eq!(s.day_totals()["2026-10-09"], 25.0);
    assert_eq!(
        s.window_start(),
        Some(new.resets_at.unwrap() - Duration::days(7))
    );
    assert_eq!(
        s.interrupted_window_start(),
        Some(old.resets_at.unwrap() - Duration::days(7))
    );
}

/// Window metadata persists, so an early reset that happened while closed is still seen.
#[test]
fn test_window_metadata_survives_relaunch_so_an_early_reset_while_closed_is_still_seen() {
    let backing = Arc::new(InMemoryStringStore::default());
    let old = window(Some(-24), 4 * 24);
    {
        let mut s = new_store(&backing);
        s.record(0.0, at(0, 9), Some(old), 7);
        s.record(40.0, at(0, 9), Some(old), 7);
    }
    let mut s = new_store(&backing);
    assert_eq!(s.window_resets_at(), old.resets_at);
    assert_eq!(s.observed_start(), old.start);
    let new = window(Some(10), 7 * 24 + 10);
    assert_eq!(s.record(3.0, at(0, 11), Some(new), 7), T::EarlyReset);
    drop(s);
    let reloaded = new_store(&backing);
    assert_eq!(reloaded.window_start(), Some(at(0, 10)));
    assert_eq!(reloaded.window_resets_at(), new.resets_at);
}

/// A legacy payload without window metadata decodes and behaves.
#[test]
fn test_payload_without_window_metadata_decodes() {
    let backing = Arc::new(InMemoryStringStore::default());
    let yesterday = to_apple_seconds(at(-1, 0));
    backing.set(
        &format!(r#"{{"days":[{yesterday},12],"lastUsedPercent":40}}"#),
        KEY,
    );
    let mut s = new_store(&backing);
    assert_eq!(s.day_totals()["2026-10-09"], 12.0);
    assert_eq!(
        s.record(45.0, at(0, 9), Some(window(Some(-24), 4 * 24)), 7),
        T::None
    );
    assert_eq!(today_total(&s), 5.0);
}

/// `clear` drops window metadata and days.
#[test]
fn test_clear_drops_window_metadata() {
    let mut s = new_store(&Arc::default());
    let w = window(Some(-24), 4 * 24);
    s.record(0.0, at(0, 9), Some(w), 7);
    s.record(10.0, at(0, 9), Some(w), 7);
    s.clear();
    assert!(s.day_totals().is_empty());
    assert_eq!(s.last_used_percent(), None);
    assert_eq!(s.window_start(), None);
    assert_eq!(s.interrupted_window_start(), None);
    assert_eq!(s.observed_start(), None);
    assert_eq!(s.window_resets_at(), None);
}

/// Classification needs `resets_at` on both sides.
#[test]
fn test_classify_needs_provider_metadata_on_both_sides() {
    let now = at(0, 9);
    let full = window(Some(0), 4 * 24);
    let none = QuotaWindow::default();
    assert_eq!(T::classify(none, Some(60.0), full, 2.0, 7, now), T::None);
    assert_eq!(T::classify(full, Some(60.0), none, 2.0, 7, now), T::None);
}

/// An early reset requires a usage drop and a known previous value.
#[test]
fn test_classify_early_reset_requires_a_usage_drop() {
    let now = at(0, 9);
    let old = window(None, 4 * 24);
    let new = window(None, 7 * 24);
    assert_eq!(
        T::classify(old, Some(60.0), new, 2.0, 7, now),
        T::EarlyReset
    );
    assert_eq!(T::classify(old, Some(60.0), new, 58.0, 7, now), T::None);
    assert_eq!(T::classify(old, None, new, 2.0, 7, now), T::None);
}

/// Reset-instant jitter is ignored; a multi-day move is not.
#[test]
fn test_classify_ignores_reset_instant_jitter() {
    let now = at(0, 9);
    let old = window(None, 4 * 24);
    let jitter = QuotaWindow {
        start: None,
        resets_at: old.resets_at.map(|r| r + Duration::seconds(30)),
    };
    assert_eq!(T::classify(old, Some(60.0), jitter, 2.0, 7, now), T::None);
    let moved = window(None, 7 * 24);
    assert_eq!(
        T::classify(old, Some(60.0), moved, 2.0, 7, now),
        T::EarlyReset
    );
}

/// A reset of a window that was about to end is still an early reset, not a rollover.
#[test]
fn test_classify_late_early_reset_is_not_mistaken_for_rollover() {
    let now = at(0, 9);
    let old = window(None, 24 + 9);
    let new = window(None, 8 * 24);
    assert_eq!(
        T::classify(old, Some(90.0), new, 1.0, 7, now),
        T::EarlyReset
    );
}

/// A rollover needs the old reset to have passed and a half-period advance.
#[test]
fn test_classify_rollover_needs_old_reset_passed_and_half_period_advance() {
    let now = at(0, 9);
    let old = window(None, 8);
    assert_eq!(
        T::classify(old, Some(60.0), window(None, 8 + 7 * 24), 2.0, 7, now),
        T::Rollover
    );
    assert_eq!(
        T::classify(old, Some(60.0), window(None, 8 + 24), 2.0, 7, now),
        T::None
    );
}

/// An unchanged sample does not rewrite the file; a change does.
#[test]
fn test_unchanged_sample_does_not_rewrite_the_file() {
    let backing = Arc::new(InMemoryStringStore::default());
    let mut s = new_store(&backing);
    s.record(10.0, at(0, 9), None, 7);
    let writes = backing.write_count();
    s.record(10.0, at(0, 9), None, 7);
    assert_eq!(backing.write_count(), writes);
    s.record(30.0, at(0, 9), None, 7);
    assert_eq!(backing.write_count(), writes + 1);
}

/// The macOS flat `days` array loads (Denver) and is rewritten with `dayTotals`.
#[test]
fn test_instant_keyed_payload_loads_and_is_rewritten() {
    let backing = Arc::new(InMemoryStringStore::default());
    let denver_midnight = |d: u32| Utc.with_ymd_and_hms(2026, 9, d, 6, 0, 0).unwrap();
    let (a, b) = (
        to_apple_seconds(denver_midnight(14)),
        to_apple_seconds(denver_midnight(15)),
    );
    backing.set(
        &format!(r#"{{"days":[{a},7,{b},4],"lastUsedPercent":35}}"#),
        KEY,
    );
    let clock = offset_clock(Utc.with_ymd_and_hms(2026, 9, 20, 18, 0, 0).unwrap(), -6);
    let s = DailyQuotaDeltaStore::new(backing.clone(), KEY, clock);
    assert_eq!(s.day_totals()["2026-09-14"], 7.0);
    assert_eq!(s.day_totals()["2026-09-15"], 4.0);
    assert_eq!(s.last_used_percent(), Some(35.0));
    let rewritten = backing.value(KEY).unwrap();
    assert!(rewritten.contains(r#""2026-09-14":7"#), "{rewritten}");
    assert!(rewritten.contains(r#""days":["#), "{rewritten}");
}

/// Window fields are Apple reference-date seconds.
#[test]
fn test_instant_keyed_payload_with_window_metadata_loads() {
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(
        r#"{"days":[],"lastUsedPercent":35,"windowStart":813093922.556,"interruptedWindowStart":812000000,"observedStart":813093922.556,"windowResetsAt":813698722.556}"#,
        KEY,
    );
    let s = new_store(&backing);
    let secs = |v: f64| tokenmon_core::storage::from_apple_seconds(v);
    assert_eq!(s.window_start(), secs(813_093_922.556));
    assert_eq!(s.window_resets_at(), secs(813_698_722.556));
    assert_eq!(s.interrupted_window_start(), secs(812_000_000.0));
    assert!(backing.value(KEY).unwrap().contains("813093922.556"));
}

/// Recorded days keep their dates after the time zone changes.
#[test]
fn test_days_keep_their_dates_after_a_time_zone_change() {
    let backing = Arc::new(InMemoryStringStore::default());
    let denver_now = Utc.with_ymd_and_hms(2026, 9, 15, 16, 0, 0).unwrap();
    {
        let mut s = DailyQuotaDeltaStore::new(backing.clone(), KEY, offset_clock(denver_now, -6));
        s.record(4.0, denver_now, None, 7);
        s.record(10.0, denver_now, None, 7);
    }
    let tokyo = DailyQuotaDeltaStore::new(backing, KEY, offset_clock(denver_now, 9));
    assert_eq!(tokyo.day_totals()["2026-09-15"], 6.0);
    let tokyo_midnight = Utc.with_ymd_and_hms(2026, 9, 14, 15, 0, 0).unwrap();
    assert_eq!(tokyo.spent_by_day().get(&tokyo_midnight), Some(&6.0));
}

/// Forty days are kept in the injected calendar: kept at +31, pruned at +41.
#[test]
fn test_prune_keeps_forty_days_in_the_injected_calendar() {
    let mut s = new_store(&Arc::default());
    s.record(0.0, at(0, 9), None, 7);
    s.record(10.0, at(0, 9), None, 7);
    s.record(20.0, at(31, 9), None, 7);
    assert!(s.day_totals().contains_key(TODAY));
    s.record(30.0, at(41, 9), None, 7);
    assert!(!s.day_totals().contains_key(TODAY));
}

/// Garbage on disk is replaced by an empty, valid payload.
#[test]
fn test_garbage_payload_is_reset() {
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set("{not json", KEY);
    let s = new_store(&backing);
    assert!(s.day_totals().is_empty());
    assert!(serde_json::from_str::<serde_json::Value>(&backing.value(KEY).unwrap()).is_ok());
}

/// Non-finite samples are ignored instead of poisoning the baseline.
#[test]
fn test_non_finite_samples_are_ignored() {
    let mut s = new_store(&Arc::default());
    s.record(10.0, at(0, 9), None, 7);
    assert_eq!(s.record(f64::NAN, at(0, 9), None, 7), T::None);
    s.record(f64::INFINITY, at(0, 9), None, 7);
    assert_eq!(s.last_used_percent(), Some(10.0));
}

/// `is_rollover`: a full-period advance is a rollover; a short, missing or backwards one is not.
#[test]
fn test_is_new_window_detects_rollover() {
    let prev = Some(at(0, 0));
    assert!(T::is_rollover(prev, Some(at(7, 0)), 7));
    assert!(!T::is_rollover(prev, Some(at(1, 0)), 7));
    assert!(!T::is_rollover(None, Some(at(7, 0)), 7));
    assert!(!T::is_rollover(Some(at(7, 0)), prev, 7));
}

/// Hostile on-disk values (huge dates, absurd keys) load without panicking.
#[test]
fn test_hostile_payload_does_not_panic() {
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(
        r#"{"dayTotals":{"9999999-01-01":1,"zzz":2,"2026-10-09":3},"days":[1e300,1],"windowStart":1e300,"windowResetsAt":-1e300}"#,
        KEY,
    );
    let s = new_store(&backing);
    assert_eq!(s.day_totals().len(), 1);
    assert_eq!(s.window_start(), None);
}
