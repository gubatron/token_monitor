//! Stable-toolchain replay of the committed fuzz corpus for the usage/util
//! parsers, applying the same invariants as the cargo-fuzz targets in `fuzz/`.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use chrono::{Datelike, FixedOffset, SecondsFormat};
use serde_json::Value;
use tokenmon_core::usage::{Calendar, DayKey};
use tokenmon_core::util::json::{decimal, first_bool, first_double, nested, number};
use tokenmon_core::util::parse_flexible;

fn corpus(target: &str) -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    let mut inputs: Vec<Vec<u8>> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("corpus dir {}: {e}", dir.display()))
        .map(|entry| std::fs::read(entry.unwrap().path()).unwrap())
        .collect();
    // Pathological inputs that should always stay handled.
    inputs.push(Vec::new());
    inputs.push(vec![0xff; 64]);
    inputs.push(b"9999-12-31T23:59:59.999999999+23:59".to_vec());
    inputs.push(b"\x00\x00\x00\x009999-12-31".to_vec());
    inputs
}

/// Mirrors `fuzz/src/bin/core_iso8601.rs`.
fn run_iso8601(data: &[u8]) {
    let text = String::from_utf8_lossy(data);
    if let Some(parsed) = parse_flexible(&text)
        && (0..=9999).contains(&parsed.year())
    {
        let again = parsed.to_rfc3339_opts(SecondsFormat::Nanos, true);
        assert_eq!(parse_flexible(&again), Some(parsed));
    }
}

fn check_json(value: &Value) {
    let n = number(Some(value));
    if let Some(n) = n {
        assert!(n.is_finite());
    }
    if decimal(Some(value)).is_some() {
        assert!(n.is_some());
    }
    if let Some(obj) = value.as_object() {
        let _ = first_double(obj, &["val", "value", "a"]);
        let _ = first_bool(obj, &["flag", "val"], false);
        let _ = nested(obj, &["a", "b", "c"]);
        for child in obj.values() {
            check_json(child);
        }
    }
}

/// Mirrors `fuzz/src/bin/core_json_number.rs`.
fn run_json(data: &[u8]) {
    if let Ok(value) = serde_json::from_slice::<Value>(data) {
        check_json(&value);
    }
    check_json(&Value::String(String::from_utf8_lossy(data).into_owned()));
}

/// Mirrors `fuzz/src/bin/core_day_key.rs`.
fn run_day_key(data: &[u8]) {
    let (head, rest) = data.split_at(data.len().min(4));
    let mut raw = [0u8; 4];
    raw[..head.len()].copy_from_slice(head);
    let secs = i32::from_le_bytes(raw).rem_euclid(26 * 3600 + 1) - 12 * 3600;
    let cal = Calendar::fixed(FixedOffset::east_opt(secs).unwrap());
    let key = DayKey::from_raw(String::from_utf8_lossy(rest).into_owned());
    if let Some(start) = key.start_of_day(&cal) {
        let canonical = DayKey::for_instant(start, &cal);
        assert_eq!(
            Some(&canonical),
            key.to_date().map(DayKey::for_date).as_ref()
        );
        let _ = key.adding_days(i64::from(secs), &cal);
        let _ = DayKey::for_stored_start_of_day(start, i64::from(secs));
    }
    let _ = key.adding_days(i64::MAX, &cal);
    let _ = key.adding_days(i64::MIN, &cal);
}

/// Every committed ISO-8601 corpus input is handled with the fuzz invariants.
#[test]
fn test_iso8601_corpus_replays() {
    for d in corpus("core_iso8601") {
        run_iso8601(&d);
    }
}

/// Every committed JSON-number corpus input is handled with the fuzz invariants.
#[test]
fn test_json_number_corpus_replays() {
    for d in corpus("core_json_number") {
        run_json(&d);
    }
}

/// Every committed day-key corpus input is handled with the fuzz invariants.
#[test]
fn test_day_key_corpus_replays() {
    for d in corpus("core_day_key") {
        run_day_key(&d);
    }
}
