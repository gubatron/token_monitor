//! Invariants: the navigation policy never panics, and only `http(s)` URLs
//! with a host (plus exactly `about:blank`) are ever allowed.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::webauth::gate::is_navigation_allowed;

fuzz_target!(|data: &str| {
    if is_navigation_allowed(data) && data != "about:blank" {
        let u = url::Url::parse(data).expect("allowed implies parseable");
        assert!(matches!(u.scheme(), "http" | "https"));
        assert!(u.host_str().is_some());
    }
});
