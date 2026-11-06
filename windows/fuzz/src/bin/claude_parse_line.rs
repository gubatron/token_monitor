#![no_main]
//! Claude Code transcript line parser: arbitrary bytes never panic and the
//! consumed-prefix, newline and count invariants hold.

use libfuzzer_sys::fuzz_target;
use tokenmon_provider_claude::{parse_complete_lines, parse_jsonl, parse_line, usage_line_ranges};

fuzz_target!(|data: &[u8]| {
    let _ = parse_line(data);
    for r in usage_line_ranges(data) {
        assert!(r.start <= r.end && r.end <= data.len());
        assert!(!data[r].contains(&b'\n'));
    }
    let (entries, consumed) = parse_complete_lines(data);
    assert!(consumed <= data.len());
    assert!(consumed == data.len() || consumed == 0 || data[consumed - 1] == b'\n');
    // Consuming a prefix parses exactly what parsing that prefix yields.
    assert_eq!(entries, parse_jsonl(&data[..consumed]));
    for e in &entries {
        assert!(e.input_tokens >= 0 && e.output_tokens >= 0 && e.cache_read_tokens >= 0);
        assert!(e.cache_write_5m_tokens >= 0 && e.cache_write_1h_tokens >= 0);
        assert_ne!(e.model, "<synthetic>");
    }
});
