//! History store (spec 02 section 5.5) and CSV/JSON export tests.
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

mod common;

use chrono::{DateTime, TimeDelta, Utc};
use common::{clock_at, utc};
use std::fs;
use tempfile::TempDir;
use tokenmon_provider_grok::export::{
    CSV_HEADER, ExportFormat, csv_escape, export, export_csv, swift_double_description,
};
use tokenmon_provider_grok::history::{
    FileBacking, GrokHistoryStore, HISTORY_FILE_NAME, RECENT_CAP,
};
use tokenmon_provider_grok::models::{
    CreditBalance, ProductUsage, WeeklyUsageSnapshot, apple_date,
};

fn store() -> GrokHistoryStore {
    GrokHistoryStore::in_memory(clock_at(utc(2026, 7, 15, 12, 0)))
}

fn file_store(dir: &TempDir) -> GrokHistoryStore {
    GrokHistoryStore::open(
        Some(Box::new(FileBacking::in_dir(dir.path()))),
        clock_at(utc(2026, 7, 15, 12, 0)),
    )
}

fn snap(at: DateTime<Utc>, used: f64) -> WeeklyUsageSnapshot {
    WeeklyUsageSnapshot::new(at, used)
}

fn used(store: &GrokHistoryStore) -> Vec<f64> {
    store.recent().iter().map(|s| s.used_percent).collect()
}

fn on_disk_rows(dir: &TempDir) -> usize {
    let text = fs::read_to_string(dir.path().join(HISTORY_FILE_NAME)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["rows"].as_array().unwrap().len()
}

/// Yesterday 20 then today 10: two rows, newest first.
#[test]
fn test_append_inserts_at_front_of_recent() {
    let mut s = store();
    s.append(snap(utc(2026, 7, 14, 12, 0), 20.0));
    s.append(snap(utc(2026, 7, 15, 12, 0), 10.0));
    assert_eq!(used(&s), [10.0, 20.0]);
}

/// Same-day samples replace each other: one row, the latest value, one on disk.
#[test]
fn test_second_same_day_sample_replaces_instead_of_prepending() {
    let dir = TempDir::new().unwrap();
    let mut s = file_store(&dir);
    for (hour, pct) in [(8, 10.0), (10, 42.0), (12, 55.0)] {
        s.append(snap(utc(2026, 7, 15, hour, 0), pct));
    }
    assert_eq!(used(&s), [55.0]);
    s.flush();
    assert_eq!(on_disk_rows(&dir), 1);
}

/// +0.01 within 60 s is noise; +5 within 60 s is kept.
#[test]
fn test_noise_filter_within_sixty_seconds() {
    let t0 = utc(2026, 7, 15, 9, 0);
    let mut s = store();
    s.append(snap(t0, 50.0));
    s.append(snap(t0 + TimeDelta::seconds(30), 50.01));
    assert_eq!(used(&s), [50.0]);
    let mut s = store();
    s.append(snap(t0, 50.0));
    s.append(snap(t0 + TimeDelta::seconds(30), 55.0));
    assert_eq!(used(&s), [55.0]);
}

/// Different days produce distinct entries.
#[test]
fn test_different_days_produce_distinct_entries() {
    let mut s = store();
    s.append(snap(utc(2026, 7, 14, 12, 0), 10.0));
    s.append(snap(utc(2026, 7, 15, 12, 0), 20.0));
    assert_eq!(s.recent().len(), 2);
}

/// 250 days appended: recent is capped at 200, the disk keeps all 250.
#[test]
fn test_recent_cap_evicts_oldest() {
    let dir = TempDir::new().unwrap();
    let mut s = file_store(&dir);
    let start = utc(2025, 1, 1, 12, 0);
    for day in 0..250 {
        s.append(snap(start + TimeDelta::days(day), (day % 90) as f64));
    }
    assert_eq!(s.recent().len(), RECENT_CAP);
    assert_eq!(s.all_snapshots().len(), 250);
    s.flush();
    assert_eq!(on_disk_rows(&dir), 250);
}

/// Clear empties recent and disk.
#[test]
fn test_clear_empties_recent_and_disk() {
    let dir = TempDir::new().unwrap();
    let mut s = file_store(&dir);
    s.append(snap(utc(2026, 7, 15, 9, 0), 10.0));
    s.flush();
    s.clear();
    assert!(s.recent().is_empty() && s.all_snapshots().is_empty());
    assert_eq!(on_disk_rows(&dir), 0);
}

/// Rows are keyed by account: same day, two accounts, two rows; switching
/// accounts switches `recent`.
#[test]
fn test_rows_are_keyed_by_account() {
    let at = utc(2026, 7, 15, 9, 0);
    let mut s = store();
    s.append(snap(at, 40.0).with_account(Some("a@x.co".into())));
    s.append(snap(at, 5.0).with_account(Some("b@x.co".into())));
    assert_eq!(s.all_snapshots().len(), 2);
    assert_eq!(s.active_account(), Some("b@x.co"));
    assert_eq!(used(&s), [5.0]);
    s.set_active_account(Some("a@x.co"));
    assert_eq!(used(&s), [40.0]);
    s.set_active_account(None);
    assert!(s.recent().is_empty());
}

/// An identical idle poll later the same day leaves the row alone; a changed
/// one replaces it.
#[test]
fn test_unchanged_usage_later_the_same_day_leaves_the_row_alone() {
    let first = utc(2026, 7, 15, 8, 0);
    let mut s = store();
    s.append(snap(first, 30.0).with_products(vec![ProductUsage::new("chat", "Chat", 30.0)]));
    s.append(
        snap(first + TimeDelta::hours(3), 30.0)
            .with_products(vec![ProductUsage::new("chat", "Chat", 30.0)]),
    );
    assert_eq!(s.recent()[0].fetched_at, first);
    s.append(snap(first + TimeDelta::hours(4), 31.0));
    assert_eq!(used(&s), [31.0]);
}

/// After a flush, a reload has the row and the store did not fail.
#[test]
fn test_history_is_saved_and_reloaded() {
    let dir = TempDir::new().unwrap();
    let mut s = file_store(&dir);
    s.append(
        snap(utc(2026, 7, 15, 9, 0), 42.0)
            .with_resets_at(Some(utc(2026, 7, 16, 18, 57)))
            .with_credits(CreditBalance::from_f64(4.1))
            .with_account(Some("a@x.co".into())),
    );
    assert!(s.is_dirty());
    s.flush();
    assert!(!s.is_dirty());
    let mut reloaded = file_store(&dir);
    reloaded.set_active_account(Some("a@x.co"));
    assert_eq!(used(&reloaded), [42.0]);
    assert!(!reloaded.store_failed());
    assert_eq!(reloaded.recent()[0], s.all_snapshots()[0]);
}

/// An unreadable file marks the store failed and is never overwritten; the new
/// row stays in memory.
#[test]
fn test_unreadable_history_is_kept() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join(HISTORY_FILE_NAME);
    fs::write(&file, "not json").unwrap();
    let mut s = file_store(&dir);
    assert!(s.store_failed());
    s.append(snap(utc(2026, 7, 15, 9, 0), 10.0));
    s.flush();
    assert_eq!(fs::read_to_string(&file).unwrap(), "not json");
    assert_eq!(s.recent().len(), 1);
}

/// A failing write leaves the store dirty so the next flush retries.
#[test]
fn test_failed_write_stays_dirty() {
    let dir = TempDir::new().unwrap();
    // A directory in place of the file makes the rename fail.
    fs::create_dir(dir.path().join(HISTORY_FILE_NAME)).unwrap();
    let mut s = GrokHistoryStore::open(
        Some(Box::new(FileBacking::in_dir(dir.path()))),
        clock_at(utc(2026, 7, 15, 12, 0)),
    );
    s.append(snap(utc(2026, 7, 15, 9, 0), 10.0));
    s.flush();
    assert!(s.is_dirty());
}

/// The persisted JSON matches the macOS shape: camelCase keys, dates as Apple
/// reference seconds, nil optionals omitted.
#[test]
fn test_persisted_format_matches_macos_shape() {
    let at = utc(2026, 7, 15, 9, 0);
    let s = snap(at, 42.0).with_products(vec![ProductUsage::new("chat", "Chat", 42.0)]);
    let json: serde_json::Value = serde_json::to_value(&s).unwrap();
    assert_eq!(json["usedPercent"], 42.0);
    assert!(json.get("resetsAt").is_none() && json.get("accountEmail").is_none());
    assert_eq!(json["products"][0]["colorToken"], "chat");
    assert_eq!(json["fetchedAt"], at.timestamp() as f64 - 978_307_200.0);
    // Decoding a macOS-style row.
    let mac = format!(
        r#"{{"id":"6F9619FF-8B86-D011-B42D-00C04FC964FF","fetchedAt":{},"usedPercent":1,"remainingPercent":99,"products":[]}}"#,
        apple_date::to_apple(at)
    );
    let decoded: WeeklyUsageSnapshot = serde_json::from_str(&mac).unwrap();
    assert_eq!(decoded.fetched_at, at);
}

/// Decoding clamps products and rejects non-finite or absurd dates.
#[test]
fn test_decode_rejects_bad_dates_and_clamps_percent() {
    let bad = r#"{"id":"6F9619FF-8B86-D011-B42D-00C04FC964FF","fetchedAt":1e300,"usedPercent":1,"remainingPercent":99,"products":[]}"#;
    assert!(serde_json::from_str::<WeeklyUsageSnapshot>(bad).is_err());
    let clamp = r#"{"id":"6F9619FF-8B86-D011-B42D-00C04FC964FF","fetchedAt":0,"usedPercent":1,"remainingPercent":99,"products":[{"id":"x","displayName":"X","percentOfPool":500,"colorToken":"other"}]}"#;
    let s: WeeklyUsageSnapshot = serde_json::from_str(clamp).unwrap();
    assert_eq!(s.products[0].percent_of_pool, 100.0);
}

fn export_fixture() -> Vec<WeeklyUsageSnapshot> {
    vec![
        snap(utc(2026, 7, 15, 9, 0), 35.0)
            .with_resets_at(Some(utc(2026, 7, 20, 0, 0)))
            .with_products(vec![
                ProductUsage::new("build", "Grok Build", 25.0),
                ProductUsage::new("api", "API", 9.5),
            ])
            .with_credits(CreditBalance::from_f64(4.1))
            .with_account(Some("user@example.com".into())),
    ]
}

/// CSV has the header, no trailing newline, and Swift-style number text.
#[test]
fn test_export_csv_contains_header_and_products() {
    let csv = export_csv(&export_fixture());
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], CSV_HEADER);
    assert!(!csv.ends_with('\n'));
    assert_eq!(
        lines[1],
        "2026-07-15T09:00:00Z,35.00,65.00,2026-07-20T00:00:00Z,build:25.0|api:9.5,4.1,user@example.com"
    );
    assert!(csv.contains("fetchedAt,usedPercent") && csv.contains("build:25"));
}

/// A formula-looking email is neutralized and quoted when needed.
#[test]
fn test_export_csv_neutralizes_formula_injection() {
    let s = snap(utc(2026, 7, 15, 9, 0), 1.0).with_account(Some("=cmd|'/C calc'!A1".into()));
    let csv = export_csv(&[s]);
    assert!(csv.contains("'=cmd|"));
    assert!(!csv.contains(",=cmd|"));
}

/// Leading tab and carriage return are guarded.
#[test]
fn test_csv_escape_guards_leading_tab_and_carriage_return() {
    assert_eq!(csv_escape("\t=1+1"), "'\t=1+1");
    assert_eq!(csv_escape("\r=1+1"), "\"'\r=1+1\"");
    assert_eq!(csv_escape("\r\n=1+1"), "\"'\r\n=1+1\"");
    assert_eq!(csv_escape("-5"), "'-5");
    assert_eq!(csv_escape("plain"), "plain");
}

/// Line breaks, commas and quotes quote the cell.
#[test]
fn test_csv_escape_quotes_carriage_return_and_line_breaks() {
    for cell in ["a\rb", "a\r\nb", "a\nb", "a,b"] {
        assert_eq!(csv_escape(cell), format!("\"{cell}\""));
    }
    assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
}

/// JSON export is an array with sorted keys, ISO dates, no id and no nils.
#[test]
fn test_export_json_round_trip() {
    let bytes = export(&export_fixture(), ExportFormat::Json).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let row = &json.as_array().unwrap()[0];
    assert_eq!(row["fetchedAt"], "2026-07-15T09:00:00Z");
    assert_eq!(row["extraCreditsBalance"], 4.1);
    assert!(row.get("id").is_none());
    let keys: Vec<&String> = row.as_object().unwrap().keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert!(text.find("accountEmail").unwrap() < text.find("usedPercent").unwrap());
    assert_eq!(export(&[], ExportFormat::Json).unwrap(), b"[]");
    assert_eq!(ExportFormat::Csv.default_file_name(), "grok-usage.csv");
}

/// Swift `Double.description` conventions.
#[test]
fn test_swift_double_description() {
    for (value, text) in [
        (25.0, "25.0"),
        (0.1, "0.1"),
        (33.333_333_333_333_336, "33.333333333333336"),
        (0.0, "0.0"),
        (100.0, "100.0"),
        (1e-5, "1e-05"),
        (1.5e-7, "1.5e-07"),
    ] {
        assert_eq!(swift_double_description(value), text);
    }
}

/// An empty history exports only the header.
#[test]
fn test_export_empty_history_is_only_the_header() {
    assert_eq!(export_csv(&[]), CSV_HEADER);
}
