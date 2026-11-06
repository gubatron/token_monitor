//! Challenge / HTML / login-page detection on arbitrary bodies and headers:
//! never panics, and the documented implications hold.
#![no_main]

use libfuzzer_sys::fuzz_target;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use tokenmon_core::net::{is_bot_challenge, looks_like_html, looks_like_login_page};

fuzz_target!(|data: &[u8]| {
    let (flag, body) = data.split_first().map_or((0, data), |(f, b)| (*f, b));
    let mut headers = HeaderMap::new();
    if flag & 1 != 0 {
        headers.insert("cf-mitigated", HeaderValue::from_static("challenge"));
    }
    let status = if flag & 2 != 0 {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::OK
    };
    let html = looks_like_html(body);
    if looks_like_login_page(body) {
        assert!(html);
    }
    let challenge = is_bot_challenge(status, &headers, body);
    assert_eq!(challenge, flag & 1 != 0 || (flag & 2 != 0 && html));
});
