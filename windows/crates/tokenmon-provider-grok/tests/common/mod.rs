//! Shared helpers for the Grok integration tests.
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]
#![allow(dead_code)]

use std::sync::Arc;

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use tokenmon_core::ManualClock;

/// Fixture A (spec 02 section 5.8): used 36, chat 23, build 13, voice enum
/// with no percent.
pub const FIXTURE_A_HEX: &str = concat!(
    "000000005f0a5d0d0000104212001a00220b08b1debfd20610b8efb07f2a0b08b1d3e4d20610b8efb07f",
    "3a070804150000b8413a07080215000050413a020806421c0802120b08b1debfd20610b8efb07f1a0b08b1d3e4d20610b8efb07f",
    "580162006801800000000f677270632d7374617475733a300d0a",
);

/// Fixture B: live capture, used 64, build 30, chat 21, imagine 12, other 1.
pub const FIXTURE_B_HEX: &str = concat!(
    "000000006d0a6b0d0000804212001a00220b08b1c889d30610b8efb07f2a0b08b1bdaed30610b8efb07f",
    "3a070802150000f0413a070804150000a8413a07080515000040413a070803150000803f",
    "421c0802120b08b1c889d30610b8efb07f1a0b08b1bdaed30610b8efb07f580162006801",
    "800000000f677270632d7374617475733a300d0a",
);

/// Decodes hex (no separators).
pub fn hex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0, "hex must have even length");
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("valid hex"))
        .collect()
}

/// `0a 05 0d <64.0f32>`: path [1,1] = 64.0, so only the product scan sees
/// any bad bytes appended after it.
pub const USED_PERCENT_64: [u8; 7] = [0x0A, 0x05, 0x0D, 0x00, 0x00, 0x80, 0x42];

/// LEB128 encoding.
pub fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// Builds a gRPC-web frame the way the Swift test helper does.
/// `products` is `(enum, optional percent)`.
pub fn synthetic_frame(used: f32, products: &[(u64, Option<f32>)], reset_at: u64) -> Vec<u8> {
    let mut inner = vec![0x0D];
    inner.extend_from_slice(&used.to_le_bytes());
    for (enum_value, percent) in products {
        let mut sub = vec![0x08];
        sub.extend(varint(*enum_value));
        if let Some(p) = percent {
            sub.push(0x15);
            sub.extend_from_slice(&p.to_le_bytes());
        }
        inner.push(0x3A);
        inner.push(u8::try_from(sub.len()).unwrap());
        inner.extend(sub);
    }
    let mut reset = vec![0x08];
    reset.extend(varint(reset_at));
    inner.push(0x2A);
    inner.push(u8::try_from(reset.len()).unwrap());
    inner.extend(reset);
    let mut payload = vec![0x0A];
    payload.extend(varint(inner.len() as u64));
    payload.extend(inner);
    let mut frame = vec![0x00];
    frame.extend((payload.len() as u32).to_be_bytes());
    frame.extend(payload);
    frame
}

/// Instant from UTC components.
pub fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
}

/// A UTC manual clock at `now`.
pub fn clock_at(now: DateTime<Utc>) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(now))
}

/// A manual clock at `now` with a fixed local offset in hours east of UTC.
pub fn clock_with_offset(now: DateTime<Utc>, hours: i32) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(now).with_offset(FixedOffset::east_opt(hours * 3600).unwrap()))
}
