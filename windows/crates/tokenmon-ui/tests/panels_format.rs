//! Tests for panel display formatting (spec 03 section 1.7, spec 02 section 1.7).
#![allow(missing_docs, clippy::unwrap_used, clippy::assert_is_empty)]

use chrono::{Duration, FixedOffset, TimeZone, Utc};
use tokenmon_ui::panels::format::*;

/// `Format.tokens` boundaries including unit promotion at 999,500.
#[test]
fn test_tokens_compact_boundaries() {
    let cases: [(u64, &str); 12] = [
        (0, "0"),
        (999, "999"),
        (1_000, "1K"),
        (1_499, "1K"),
        (1_500, "2K"),
        (999_499, "999K"),
        (999_500, "1.0M"),
        (9_949_999, "9.9M"),
        (9_950_000, "10M"),
        (12_400_000, "12M"),
        (999_600_000, "1.0B"),
        (2_500_000_000, "2.5B"),
    ];
    for (n, want) in cases {
        assert_eq!(tokens(n), want, "tokens({n})");
    }
}

/// en_US currency with grouping and rounding.
#[test]
fn test_usd_formats_like_en_us_currency() {
    assert_eq!(usd(4.1), "$4.10");
    assert_eq!(usd(410.0), "$410.00");
    assert_eq!(usd(1_234_567.891), "$1,234,567.89");
    assert_eq!(usd(62.02), "$62.02");
    assert_eq!(usd(f64::NAN), "$0.00");
    assert_eq!(usd(-3.5), "-$3.50");
}

/// Percent labels round half away from zero and clamp.
#[test]
fn test_percent_used_rounds_and_clamps() {
    assert_eq!(percent_used(35.4), "35% Used");
    assert_eq!(percent_used(35.5), "36% Used");
    assert_eq!(percent_used(140.0), "100% Used");
    assert_eq!(percent_used(-4.0), "0% Used");
    assert_eq!(percent_label(-3.0), "0%");
    assert_eq!(percent_label(2.5), "3%");
}

/// Reset caption uses `EEE dd MMMM h:mma` with lowercase am/pm, in the local zone.
#[test]
fn test_reset_caption_examples_from_the_spec() {
    let utc = FixedOffset::east_opt(0).unwrap();
    assert_eq!(
        reset_caption(Utc.with_ymd_and_hms(2026, 7, 16, 20, 25, 0).unwrap(), utc),
        "Resets Thu 16 July 8:25pm"
    );
    assert_eq!(
        reset_caption(Utc.with_ymd_and_hms(2026, 8, 27, 11, 0, 0).unwrap(), utc),
        "Resets Thu 27 August 11:00am"
    );
    assert_eq!(
        reset_caption(Utc.with_ymd_and_hms(2026, 8, 5, 0, 0, 0).unwrap(), utc),
        "Resets Wed 05 August 12:00am"
    );
    let mdt = FixedOffset::west_opt(6 * 3600).unwrap();
    assert_eq!(
        reset_caption(Utc.with_ymd_and_hms(2026, 8, 24, 6, 0, 0).unwrap(), mdt),
        "Resets Mon 24 August 12:00am"
    );
    assert_eq!(
        reset_caption_short(Utc.with_ymd_and_hms(2026, 8, 24, 0, 0, 0).unwrap(), utc),
        "Resets Mon 12:00am"
    );
}

/// Relative captions and ages.
#[test]
fn test_relative_and_age_text() {
    let now = Utc.with_ymd_and_hms(2026, 7, 16, 10, 0, 0).unwrap();
    assert_eq!(
        reset_caption_relative(now + Duration::minutes(130), now),
        "Resets in 2 hours"
    );
    assert_eq!(
        reset_caption_relative(now + Duration::hours(1), now),
        "Resets in 1 hour"
    );
    assert_eq!(
        reset_caption_relative(now - Duration::hours(1), now),
        "Resets in 0 seconds"
    );
    assert_eq!(age_text(now - Duration::minutes(5), now), "5 min");
    assert_eq!(age_text(now - Duration::hours(2), now), "2 hr");
    assert_eq!(age_text(now - Duration::days(1), now), "1 day");
    assert_eq!(age_text(now - Duration::days(3), now), "3 days");
    assert_eq!(age_text(now + Duration::days(3), now), "0 sec");
}
