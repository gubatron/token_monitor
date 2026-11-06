#![no_main]
//! URL trust checks never panic, and an accepted repository download has the
//! exact shape (https, github.com, six clean segments, no dot segments).

use libfuzzer_sys::fuzz_target;
use tokenmon_core::update::{is_repository_release_download, is_trusted_download};

fuzz_target!(|data: &[u8]| {
    let Ok(url) = std::str::from_utf8(data) else {
        return;
    };
    let repo = is_repository_release_download(url, "gubatron", "token_monitor");
    let trusted = is_trusted_download(url, "gubatron", "token_monitor");
    assert!(!repo || trusted);
    if repo {
        assert!(url.to_ascii_lowercase().starts_with("https://github.com"));
        assert!(!url.contains(['\\', '?', '#']));
        // `@` is legal in a tag name (path) but never in the authority.
        let authority = url.split_once("://").map_or("", |(_, r)| r.split('/').next().unwrap_or(""));
        assert!(!authority.contains('@'));
        let rest = url.split_once("://").map_or("", |(_, r)| r);
        let segments: Vec<&str> = rest.split('/').skip(1).collect();
        assert_eq!(segments.len(), 6);
        assert!(segments.iter().all(|s| !s.is_empty() && *s != "." && *s != ".."));
    }
});
