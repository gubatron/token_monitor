#![no_main]
//! `AppVersion::parse` never panics; ordering is a consistent total order and
//! `Display` output reparses to an equal version.

use libfuzzer_sys::fuzz_target;
use tokenmon_core::update::AppVersion;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (a, b) = text.split_once('\n').unwrap_or((text, text));
    let Some(x) = AppVersion::parse(a) else {
        return;
    };
    assert_eq!(AppVersion::parse(&x.to_string()), Some(x.clone()));
    assert_eq!(x.cmp(&x), std::cmp::Ordering::Equal);
    if let Some(y) = AppVersion::parse(b) {
        assert_eq!(x.cmp(&y), y.cmp(&x).reverse());
    }
});
