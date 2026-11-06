#![no_main]
//! `Domain::matches` agrees with an independent oracle (equal to a host, or a
//! dot-separated subdomain of one, ignoring case and surrounding dots) and
//! never accepts `notX` or `X.evil` look-alikes.

use libfuzzer_sys::fuzz_target;
use tokenmon_core::net::Domain;

const HOSTS: [&str; 3] = ["claude.ai", "chatgpt.com", "x.ai"];

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let lowered = text.to_lowercase();
    let d = lowered.trim_matches('.');
    let expected = HOSTS
        .iter()
        .any(|h| d == *h || d.ends_with(&format!(".{h}")));
    assert_eq!(Domain::matches(&text, &HOSTS), expected);
    for h in HOSTS {
        assert!(!Domain::matches(&format!("not{h}"), &HOSTS));
        assert!(!Domain::matches(&format!("{h}.evil.com"), &HOSTS));
        assert!(Domain::matches(&format!("www.{h}"), &HOSTS));
    }
});
