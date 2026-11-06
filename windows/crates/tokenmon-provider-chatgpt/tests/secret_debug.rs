//! #43: cookie and token holders never print their secrets through `Debug`.
#![allow(clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use tokenmon_provider_chatgpt::{AccessTokenCache, RawReply};

const SECRET: &str = "SUPER-SECRET-VALUE-123";

#[test]
fn raw_reply_debug_hides_cookies_and_body() {
    let r = RawReply {
        body: SECRET.as_bytes().to_vec(),
        set_cookies: vec![format!("__Secure-next-auth.session-token={SECRET}; Path=/")],
    };
    assert!(!format!("{r:?}").contains(SECRET));
}

#[test]
fn access_token_cache_debug_hides_token_and_key() {
    let cache = AccessTokenCache::new();
    let until = Utc.timestamp_opt(1_790_000_000, 0).unwrap();
    cache.store(SECRET, SECRET, until);
    assert!(!format!("{cache:?}").contains(SECRET));
}
