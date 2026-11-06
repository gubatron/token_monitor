//! Ports of `HourlyDeltaActivityStoreTests` (spec 01 section 1.3.4 and 7).

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use common::{at, utc_clock};
use tokenmon_core::storage::{
    HourlyDeltaActivityStore, InMemoryStringStore, TextStore, to_apple_seconds,
};

const KEY: &str = "grok_hourly_today";

fn new_store(backing: &Arc<InMemoryStringStore>, key: &str) -> HourlyDeltaActivityStore {
    HourlyDeltaActivityStore::new(backing.clone(), key, utc_clock(at(0, 12)))
}

/// Growth is credited to the sampled hour.
#[test]
fn test_growth_within_a_window_is_attributed_to_the_sampled_hour() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(10.0, at(0, 9));
    s.record(25.0, at(0, 9));
    assert_eq!(s.hour_weights()[9], 15.0);
}

/// A reset credits the post-reset value.
#[test]
fn test_window_reset_attributes_post_reset_value_instead_of_dropping_it() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(95.0, at(0, 9));
    s.record(15.0, at(0, 10));
    assert_eq!(s.hour_weights()[10], 15.0);
}

/// Tiny deltas and small downward ticks are noise.
#[test]
fn test_tiny_delta_and_small_downward_noise_are_ignored() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(10.0, at(0, 9));
    s.record(10.01, at(0, 9));
    s.record(9.9, at(0, 9));
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 0.0);
}

/// Different storage keys do not collide.
#[test]
fn test_different_keys_do_not_collide_on_disk() {
    let backing = Arc::new(InMemoryStringStore::default());
    let mut a = new_store(&backing, "a");
    let mut b = new_store(&backing, "b");
    a.record(0.0, at(0, 9));
    a.record(10.0, at(0, 9));
    b.record(0.0, at(0, 9));
    b.record(1.0, at(0, 9));
    assert_eq!(new_store(&backing, "a").hour_weights()[9], 10.0);
    assert_eq!(new_store(&backing, "b").hour_weights()[9], 1.0);
}

/// `clear` zeros the series; the next sample is a baseline.
#[test]
fn test_clear_resets_series_and_baseline() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(0.0, at(0, 9));
    s.record(10.0, at(0, 9));
    s.clear(at(0, 10));
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 0.0);
    s.record(3.0, at(0, 10));
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 0.0);
    s.record(10.0, at(0, 10));
    assert_eq!(s.hour_weights()[10], 7.0);
}

/// After `begin_new_window` a rising first sample is credited whole.
#[test]
fn test_begin_new_window_credits_rising_first_sample() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(20.0, at(0, 9));
    s.begin_new_window(false);
    s.record(30.0, at(0, 9));
    assert_eq!(s.hour_weights()[9], 30.0);
}

/// `keeping_hours` preserves today's series across an early reset.
#[test]
fn test_begin_new_window_keeping_hours_preserves_todays_series() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(0.0, at(0, 9));
    s.record(15.0, at(0, 9));
    s.begin_new_window(true);
    s.record(4.0, at(0, 11));
    assert_eq!(s.hour_weights()[9], 15.0);
    assert_eq!(s.hour_weights()[11], 4.0);
}

/// An unchanged sample does not rewrite the file.
#[test]
fn test_unchanged_sample_does_not_rewrite_the_file() {
    let backing = Arc::new(InMemoryStringStore::default());
    let mut s = new_store(&backing, KEY);
    s.record(10.0, at(0, 9));
    let writes = backing.write_count();
    s.record(10.0, at(0, 9));
    assert_eq!(backing.write_count(), writes);
    s.record(20.0, at(0, 9));
    assert_eq!(backing.write_count(), writes + 1);
}

/// The baseline carries across midnight; growth lands in the new day's hour 0.
#[test]
fn test_first_delta_after_midnight_is_credited() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(10.0, at(0, 23) + chrono::Duration::minutes(55));
    s.record(17.0, at(1, 0) + chrono::Duration::minutes(5));
    assert_eq!(s.hour_weights()[0], 7.0);
    assert_eq!(s.day_start(), at(1, 0));
}

/// A gap of several days drops the baseline, so only the later delta counts.
#[test]
fn test_gap_of_several_days_starts_without_baseline() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(10.0, at(0, 9));
    s.record(50.0, at(2, 9));
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 0.0);
    s.record(53.0, at(2, 10));
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 3.0);
}

fn legacy_payload(day_start_secs: f64) -> String {
    let weights: Vec<String> = (0..24)
        .map(|h| if h == 3 { "5".into() } else { "0".into() })
        .collect();
    format!(
        r#"{{"dayStart":{day_start_secs},"hourWeights":[{}],"lastUsedPercent":30}}"#,
        weights.join(",")
    )
}

/// An old payload without `dayKey` restores today's weights and is rewritten with it.
#[test]
fn test_instant_keyed_payload_restores_today_and_is_rewritten() {
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(&legacy_payload(to_apple_seconds(at(0, 0))), KEY);
    let mut s = new_store(&backing, KEY);
    assert_eq!(s.hour_weights()[3], 5.0);
    s.record(33.0, at(0, 15));
    assert_eq!(s.hour_weights()[15], 3.0);
    assert!(
        backing
            .value(KEY)
            .unwrap()
            .contains(r#""dayKey":"2026-10-10""#)
    );
}

/// A payload from another day starts fresh.
#[test]
fn test_instant_keyed_payload_from_another_day_starts_fresh() {
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(&legacy_payload(to_apple_seconds(at(-1, 0))), KEY);
    let s = new_store(&backing, KEY);
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 0.0);
    assert_eq!(s.last_used_percent(), None);
}

/// A payload with the wrong number of weights starts fresh instead of panicking.
#[test]
fn test_payload_with_wrong_weight_count_starts_fresh() {
    let backing = Arc::new(InMemoryStringStore::default());
    backing.set(
        r#"{"dayKey":"2026-10-10","dayStart":1,"hourWeights":[1,2,3]}"#,
        KEY,
    );
    let s = new_store(&backing, KEY);
    assert_eq!(s.hour_weights().iter().sum::<f64>(), 0.0);
}

/// The persisted JSON has the macOS shape: camelCase keys, whole numbers as integers.
#[test]
fn test_persisted_json_shape_matches_macos() {
    let backing = Arc::new(InMemoryStringStore::default());
    let mut s = new_store(&backing, KEY);
    s.record(30.0, at(0, 9));
    let value: serde_json::Value = serde_json::from_str(&backing.value(KEY).unwrap()).unwrap();
    assert_eq!(value["dayKey"], "2026-10-10");
    assert_eq!(value["dayStart"].as_f64(), Some(to_apple_seconds(at(0, 0))));
    assert_eq!(value["hourWeights"].as_array().unwrap().len(), 24);
    assert_eq!(value["lastUsedPercent"], 30);
}

/// Non-finite samples are ignored.
#[test]
fn test_non_finite_samples_are_ignored() {
    let mut s = new_store(&Arc::default(), KEY);
    s.record(10.0, at(0, 9));
    s.record(f64::NAN, at(0, 9));
    assert_eq!(s.last_used_percent(), Some(10.0));
}
