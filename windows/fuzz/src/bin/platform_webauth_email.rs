//! Invariants: the e-mail and percent-decode helpers never panic; anything
//! accepted as plausible is within the documented size and character rules.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tokenmon_platform::webauth::cookies::{
    WebCookie, extract_email, is_plausible_email, percent_decode,
};

fuzz_target!(|data: &str| {
    let decoded = percent_decode(data);
    let _ = is_plausible_email(data);
    if is_plausible_email(&decoded) {
        assert!(decoded.chars().count() <= 254);
        assert_eq!(decoded.matches('@').count(), 1);
        assert!(!decoded.contains(char::is_whitespace));
    }
    let cookie = WebCookie::new("email", data, "example.com");
    if let Some(e) = extract_email(&[cookie]) {
        assert!(is_plausible_email(&e));
    }
});
