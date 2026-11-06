//! Invariants: the escaped cell never starts with `= + - @ TAB CR` (outside the
//! `'` guard, or inside quotes after `'`) and is exactly one CSV field per
//! RFC 4180: quoted with balanced quotes, or free of separators and quotes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_grok::export::csv_escape;

fuzz_target!(|data: &[u8]| {
    let value = String::from_utf8_lossy(data);
    let out = csv_escape(&value);
    let quoted = out.len() >= 2 && out.starts_with('"') && out.ends_with('"');
    let inner = if quoted { &out[1..out.len() - 1] } else { out.as_str() };
    assert!(!matches!(
        inner.chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    ));
    if quoted {
        assert_eq!(inner.replace("\"\"", "").matches('"').count(), 0);
    } else {
        assert!(!out.contains([',', '"', '\n', '\r']));
    }
});
