//! Port of Swift `FormatTests` and `JSONTests`, plus ISO-8601, Apple epoch and AppLog.

#![allow(
    clippy::unwrap_used,
    clippy::needless_pass_by_value,
    clippy::useless_vec,
    clippy::unreadable_literal,
    clippy::float_cmp
)]

use chrono::{FixedOffset, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::str::FromStr;
use tokenmon_core::util::apple_epoch::{from_reference_seconds, to_reference_seconds};
use tokenmon_core::util::json::{
    decimal, first_bool, first_decimal, first_double, first_string, nested, number, string,
};
use tokenmon_core::util::{Format, LogCategory, parse_flexible, user_agent};

fn obj(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().cloned().expect("object fixture")
}

fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).unwrap()
}

// ---- Format ----

/// Below one thousand shows the bare count.
#[test]
fn test_tokens_below_thousand_shows_bare_count() {
    assert_eq!(Format::tokens(0), "0");
    assert_eq!(Format::tokens(999), "999");
}

/// Thousands.
#[test]
fn test_tokens_thousands() {
    assert_eq!(Format::tokens(1_000), "1K");
    assert_eq!(Format::tokens(12_400), "12K");
}

/// Millions keep one decimal below ten.
#[test]
fn test_tokens_millions_rounds_to_one_decimal_below_ten() {
    assert_eq!(Format::tokens(1_000_000), "1.0M");
    assert_eq!(Format::tokens(1_234_567), "1.2M");
}

/// Millions are whole at ten and above.
#[test]
fn test_tokens_millions_rounds_whole_at_ten_plus() {
    assert_eq!(Format::tokens(10_000_000), "10M");
    assert_eq!(Format::tokens(123_000_000), "123M");
}

/// Billions.
#[test]
fn test_tokens_billions() {
    assert_eq!(Format::tokens(1_000_000_000), "1.0B");
    assert_eq!(Format::tokens(2_500_000_000), "2.5B");
}

/// Values that round to 1000 of a unit roll into the next.
#[test]
fn test_tokens_roll_to_next_unit_at_rounding_boundary() {
    assert_eq!(Format::tokens(999_499), "999K");
    assert_eq!(Format::tokens(999_500), "1.0M");
    assert_eq!(Format::tokens(9_949_999), "9.9M");
    assert_eq!(Format::tokens(9_950_000), "10M");
    assert_eq!(Format::tokens(999_499_999), "999M");
    assert_eq!(Format::tokens(999_500_000), "1.0B");
}

/// Halves round away from zero.
#[test]
fn test_tokens_round_half_away_from_zero() {
    assert_eq!(Format::tokens(1_500), "2K");
    assert_eq!(Format::tokens(2_500), "3K");
    assert_eq!(Format::tokens(1_250_000), "1.3M");
}

/// Extremes never panic.
#[test]
fn test_tokens_extremes_do_not_panic() {
    for v in [i64::MIN, -1, i64::MAX] {
        let _ = Format::tokens(v);
    }
}

/// USD currency strings.
#[test]
fn test_usd_formats_currency_without_tilde() {
    assert_eq!(Format::usd(4.1), "$4.10");
    assert_eq!(Format::usd(0.0), "$0.00");
    assert_eq!(Format::usd(1234567.891), "$1,234,567.89");
    assert_eq!(Format::usd(-2.0), "-$2.00");
    assert_eq!(Format::usd(f64::NAN), "$0");
}

/// Issue #59: rounding follows `NumberFormatter` (half-even on the shortest
/// decimal), not the binary value. Expected strings were measured on macOS.
#[test]
fn test_usd_rounds_half_even_on_the_decimal_value() {
    let table = [
        (0.015, "$0.02"),
        (1.005, "$1.00"),
        (1.015, "$1.02"),
        (0.125, "$0.12"),
        (0.025, "$0.02"),
        (0.035, "$0.04"),
        (0.045, "$0.04"),
        (0.055, "$0.06"),
        (2.675, "$2.68"),
        (0.075, "$0.08"),
        (0.005, "$0.00"),
        (0.0049, "$0.00"),
        (0.994999, "$0.99"),
        (0.995, "$1.00"),
        (999.995, "$1,000.00"),
        (0.1 + 0.2, "$0.30"),
        (5e-324, "$0.00"),
        (1_000_000_000_000_000.5, "$1,000,000,000,000,000.50"),
        (1e21, "$1,000,000,000,000,000,000,000.00"),
        (1_234_567.891, "$1,234,567.89"),
    ];
    for (v, expected) in table {
        assert_eq!(Format::usd(v), expected, "{v}");
    }
}

/// Negative values keep their sign even when they round to zero, as on the Mac.
#[test]
fn test_usd_negative_values_keep_sign_at_zero() {
    assert_eq!(Format::usd(-0.001), "-$0.00");
    assert_eq!(Format::usd(-0.005), "-$0.00");
    assert_eq!(Format::usd(-0.015), "-$0.02");
    assert_eq!(Format::usd(-0.0), "-$0.00");
    assert_eq!(Format::usd(-1234.5), "-$1,234.50");
}

/// Twelve-hour labels.
#[test]
fn test_hour_label_twelve_hour_clock() {
    let want = [
        (0, "12a"),
        (12, "12p"),
        (5, "5a"),
        (11, "11a"),
        (13, "1p"),
        (15, "3p"),
        (23, "11p"),
    ];
    for (h, s) in want {
        assert_eq!(Format::hour_label(h), s);
    }
}

/// Reset dates lowercase the meridian.
#[test]
fn test_reset_date_matches_pattern_and_lowercases_meridian() {
    let t = Utc.with_ymd_and_hms(2026, 7, 16, 18, 57, 0).unwrap();
    assert_eq!(Format::reset_date(t, "EEE h:mma", utc()), "Thu 6:57pm");
    assert_eq!(Format::date(t, "EEE h:mma", utc()), "Thu 6:57PM");
}

/// The requested offset is used, never the host's.
#[test]
fn test_reset_date_uses_the_requested_time_zone() {
    let t = Utc.with_ymd_and_hms(2026, 7, 16, 18, 57, 0).unwrap();
    let tokyo = FixedOffset::east_opt(9 * 3600).unwrap();
    assert_eq!(Format::reset_date(t, "HH:mm", utc()), "18:57");
    assert_eq!(Format::reset_date(t, "HH:mm", tokyo), "03:57");
    assert_eq!(Format::reset_date(t, "dd HH:mm", tokyo), "17 03:57");
}

/// Default caption, plus the patterns the app uses.
#[test]
fn test_reset_caption_and_app_patterns() {
    let t = Utc.with_ymd_and_hms(2026, 7, 16, 18, 57, 0).unwrap();
    assert_eq!(
        Format::reset_caption(t, Format::DEFAULT_RESET_PATTERN, utc()),
        "Resets Thu 16 July 6:57pm"
    );
    assert_eq!(Format::date(t, "MMMM d", utc()), "July 16");
    assert_eq!(Format::date(t, "EEEE, MMM d", utc()), "Thursday, Jul 16");
    assert_eq!(Format::date(t, "EEE, MMM d", utc()), "Thu, Jul 16");
    assert_eq!(Format::date(t, "'at' H 'o''clock'", utc()), "at 18 o'clock");
}

/// 12 a.m. renders as 12.
#[test]
fn test_date_midnight_is_twelve_am() {
    let t = Utc.with_ymd_and_hms(2026, 7, 16, 0, 5, 0).unwrap();
    assert_eq!(Format::date(t, "h:mma", utc()), "12:05AM");
}

// ---- ISO-8601 ----

/// Plain and fractional forms.
#[test]
fn test_parse_flexible_plain_and_fractional() {
    let plain = parse_flexible("2026-07-16T20:25:00Z").unwrap();
    assert_eq!(plain.timestamp(), 1_784_233_500);
    let frac = parse_flexible("2026-07-16T20:25:00.123Z").unwrap();
    assert_eq!(frac.timestamp(), 1_784_233_500);
    assert_eq!(frac.timestamp_subsec_millis(), 123);
}

/// Garbage, date-only and zone-less strings are rejected.
#[test]
fn test_parse_flexible_rejects_garbage() {
    for s in ["not a date", "2026-07-16", "2026-07-16T20:25:00", "", "Z"] {
        assert!(parse_flexible(s).is_none(), "{s}");
    }
}

/// Offsets in both forms and microsecond precision.
#[test]
fn test_parse_flexible_handles_offsets_and_microseconds() {
    let a = parse_flexible("2026-07-16T14:25:00-06:00").unwrap();
    assert_eq!(a.timestamp(), 1_784_233_500);
    let b = parse_flexible("2026-07-17T01:55:00+0530").unwrap();
    assert_eq!(b.timestamp(), 1_784_233_500);
    let c = parse_flexible("2026-07-16T20:25:00.123456+00:00").unwrap();
    assert_eq!(c.timestamp_subsec_micros(), 123_456);
}

// ---- Apple epoch, logging ----

/// Apple reference seconds map to the expected instants.
#[test]
fn test_apple_epoch_round_trip() {
    let t = from_reference_seconds(811_144_800.0).unwrap();
    assert_eq!(t, Utc.with_ymd_and_hms(2026, 9, 15, 6, 0, 0).unwrap());
    assert_eq!(to_reference_seconds(t), 811_144_800.0);
    assert!(from_reference_seconds(f64::INFINITY).is_none());
    assert!(from_reference_seconds(1e300).is_none());
}

/// Identity strings.
#[test]
fn test_app_log_identity() {
    assert_eq!(user_agent(Some("1.2.1")), "TokenMon/1.2.1");
    assert_eq!(user_agent(None), "TokenMon/unknown");
    assert_eq!(LogCategory::Cache.target(), "com.modelmonitor.app::Cache");
}

// ---- JSON ----

/// Numbers from various types.
#[test]
fn test_number_from_various_types() {
    assert_eq!(number(Some(&json!(42))), Some(42.0));
    assert_eq!(number(Some(&json!(3.5))), Some(3.5));
    assert_eq!(number(Some(&json!("12.5"))), Some(12.5));
    assert_eq!(number(Some(&json!(" 7 "))), Some(7.0));
    assert_eq!(number(Some(&json!("not a number"))), None);
    assert_eq!(number(None), None);
    assert_eq!(number(Some(&json!(null))), None);
    assert_eq!(number(Some(&json!([1]))), None);
}

/// Booleans and non-finite values are rejected.
#[test]
fn test_number_rejects_booleans_and_non_finite_values() {
    for v in [
        json!(true),
        json!(false),
        json!("NaN"),
        json!("inf"),
        json!("1e400"),
    ] {
        assert_eq!(number(Some(&v)), None, "{v}");
    }
    let d = obj(json!({"flag": true, "one": 1}));
    assert_eq!(number(d.get("one")), Some(1.0));
    assert_eq!(first_double(&d, &["flag"]), None);
}

/// Wrapper objects are unwrapped one level.
#[test]
fn test_number_descends_into_val_and_value_dicts() {
    assert_eq!(number(Some(&json!({"val": 12.0}))), Some(12.0));
    assert_eq!(number(Some(&json!({"value": 88.5}))), Some(88.5));
    let n = json!({"outer": {"val": 3.0}});
    assert_eq!(number(n.get("val")), None);
    assert_eq!(number(n.get("outer")), Some(3.0));
}

/// Only strings are strings.
#[test]
fn test_string() {
    assert_eq!(string(Some(&json!("abc"))), Some("abc"));
    assert_eq!(string(Some(&json!(5))), None);
    assert_eq!(string(None), None);
}

/// Nested traversal.
#[test]
fn test_nested_traverses_keys() {
    let d = obj(json!({"a": {"b": {"c": 1.0}}}));
    assert_eq!(nested(&d, &["a", "b", "c"]), Some(&json!(1.0)));
    assert_eq!(nested(&d, &["a", "b", "missing"]), None);
    assert_eq!(nested(&d, &["nope", "b"]), None);
    assert_eq!(nested(&d, &[]), None);
}

/// First numeric key wins, in order.
#[test]
fn test_first_double_tries_keys_in_order() {
    let d = obj(json!({"percentOfPool": 25.0, "percent": 90.0}));
    let keys = ["percentOfPool", "percent", "usagePercent", "value"];
    assert_eq!(first_double(&d, &keys), Some(25.0));
    let only = obj(json!({"percent": 33.0}));
    assert_eq!(
        first_double(&only, &["percentOfPool", "percent"]),
        Some(33.0)
    );
    assert_eq!(first_double(&obj(json!({"x": 1.0})), &["a", "b"]), None);
}

/// First string key wins.
#[test]
fn test_first_string() {
    let d = obj(json!({"id": "foo", "name": "bar"}));
    assert_eq!(first_string(&d, &["id", "name"]), Some("foo"));
    assert_eq!(first_string(&d, &["name"]), Some("bar"));
    assert_eq!(first_string(&obj(json!({})), &["id"]), None);
}

/// First decimal key wins.
#[test]
fn test_first_decimal() {
    let d = obj(json!({"value": 12.0, "val": 99.0}));
    assert_eq!(
        first_decimal(&d, &["value", "val"]),
        Some(Decimal::from(12))
    );
}

/// Money keeps its written digits.
#[test]
fn test_first_decimal_keeps_money_precision() {
    let d: serde_json::Map<String, Value> = serde_json::from_str(
        r#"{"n":19.99,"s":"1234567.89","b":true,"bad":"1.5abc","w":{"val":"0.1"}}"#,
    )
    .unwrap();
    let dec = |s| Some(Decimal::from_str(s).unwrap());
    assert_eq!(first_decimal(&d, &["n"]), dec("19.99"));
    assert_eq!(first_decimal(&d, &["s"]), dec("1234567.89"));
    assert_eq!(first_decimal(&d, &["w"]), dec("0.1"));
    assert_eq!(first_decimal(&d, &["b"]), None);
    assert_eq!(first_decimal(&d, &["bad"]), None);
    assert_eq!(decimal(Some(&json!("NaN"))), None);
    assert_eq!(first_decimal(&d, &["missing", "n"]), dec("19.99"));
}

/// Booleans, numeric flags and fallback.
#[test]
fn test_first_bool() {
    assert!(first_bool(&obj(json!({"flag": true})), &["flag"], false));
    assert!(!first_bool(&obj(json!({"flag": false})), &["flag"], true));
    assert!(first_bool(&obj(json!({"n": 1})), &["n"], false));
    assert!(!first_bool(&obj(json!({"n": 0})), &["n"], true));
    assert!(first_bool(&obj(json!({})), &["missing"], true));
    assert!(first_bool(
        &obj(json!({"snake": true})),
        &["camel", "snake"],
        false
    ));
}

// ---- Percent and the currency helper from the Grok shared-helper tests ----

/// Percent clamp bounds.
#[test]
fn test_percent_clamp_bounds() {
    use tokenmon_core::usage::Percent;
    assert_eq!(Percent::clamp(0.0), 0.0);
    assert_eq!(Percent::clamp(100.0), 100.0);
    assert_eq!(Percent::clamp(-5.0), 0.0);
    assert_eq!(Percent::clamp(140.0), 100.0);
    assert_eq!(Percent::clamp(37.6), 37.6);
    assert_eq!(Percent::clamp(f64::NAN), 0.0);
    assert_eq!(Percent::clamp(f64::INFINITY), 100.0);
}

/// Whole-dollar amounts keep two decimals.
#[test]
fn test_usd_currency_formatter() {
    assert_eq!(Format::usd(410.0), "$410.00");
    assert_eq!(Format::usd(4.1), "$4.10");
}
