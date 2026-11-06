#![no_main]
//! Fuzzes `DayKey` parsing (keys come from disk) and day arithmetic. Must never
//! panic; a key that has a start of day re-keys to its canonical form.

use chrono::FixedOffset;
use libfuzzer_sys::fuzz_target;
use tokenmon_core::usage::{Calendar, DayKey};

fuzz_target!(|data: &[u8]| {
    let (head, rest) = data.split_at(data.len().min(4));
    let mut raw = [0u8; 4];
    raw[..head.len()].copy_from_slice(head);
    let secs = i32::from_le_bytes(raw).rem_euclid(26 * 3600 + 1) - 12 * 3600;
    let cal = Calendar::fixed(FixedOffset::east_opt(secs).expect("offset in range"));
    let key = DayKey::from_raw(String::from_utf8_lossy(rest).into_owned());
    if let Some(start) = key.start_of_day(&cal) {
        let canonical = DayKey::for_instant(start, &cal);
        assert_eq!(Some(&canonical), key.to_date().map(DayKey::for_date).as_ref());
        let _ = key.adding_days(i64::from(secs), &cal);
        let _ = DayKey::for_stored_start_of_day(start, i64::from(secs));
    }
    let _ = key.adding_days(i64::MAX, &cal);
    let _ = key.adding_days(i64::MIN, &cal);
});
