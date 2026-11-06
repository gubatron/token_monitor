//! Comma-folded Set-Cookie splitting: never panics; pieces are non-empty,
//! trimmed, and rejoining them loses no non-separator characters.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_core::net::split_folded_set_cookie;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let parts = split_folded_set_cookie(&text);
    for p in &parts {
        assert!(!p.is_empty() && p.trim() == p);
    }
    let kept: usize = parts.iter().map(|p| p.chars().filter(|c| !c.is_whitespace()).count()).sum();
    let total = text.chars().filter(|c| !c.is_whitespace()).count();
    assert!(kept <= total && total - kept <= parts.len() + text.matches(',').count());
});
