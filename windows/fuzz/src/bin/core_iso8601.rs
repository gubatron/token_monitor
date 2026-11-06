#![no_main]
//! Fuzzes the flexible ISO-8601 parser. Must never panic; a parsed value in
//! the four-digit-year range must survive an RFC 3339 re-format round trip.

use chrono::{Datelike, SecondsFormat};
use libfuzzer_sys::fuzz_target;
use tokenmon_core::util::parse_flexible;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Some(parsed) = parse_flexible(&text)
        && (0..=9999).contains(&parsed.year())
    {
        let again = parsed.to_rfc3339_opts(SecondsFormat::Nanos, true);
        assert_eq!(parse_flexible(&again), Some(parsed));
    }
});
