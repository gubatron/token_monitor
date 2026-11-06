//! #43: types that carry credentials never print them through `Debug`.
#![allow(clippy::unwrap_used)]

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue, SET_COOKIE};
use tokenmon_core::auth_session::RefreshedCookie;
use tokenmon_core::net::HttpResponse;
use tokenmon_core::{CapturedSession, CredentialKey, CredentialStore, InMemoryCredentialStore};

const SECRET: &str = "SUPER-SECRET-VALUE-123";

fn assert_clean(rendered: &str) {
    assert!(!rendered.contains(SECRET), "leaked: {rendered}");
}

#[test]
fn captured_session_debug_redacts_cookie_header() {
    let s = CapturedSession {
        cookie_header: format!("sessionKey={SECRET}"),
        email: Some("a@b.c".into()),
    };
    assert_clean(&format!("{s:?}"));
    assert_clean(&format!("{s:#?}"));
}

#[test]
fn refreshed_cookie_debug_redacts_value() {
    let c = RefreshedCookie {
        name: "sessionKey".into(),
        value: SECRET.into(),
        live: true,
    };
    let out = format!("{c:?}");
    assert_clean(&out);
    assert!(out.contains("sessionKey"));
}

#[test]
fn in_memory_store_debug_redacts_values() {
    let store = InMemoryCredentialStore::default();
    store.set(CredentialKey::Session, SECRET).unwrap();
    assert_clean(&format!("{store:?}"));
}

#[test]
fn http_response_debug_redacts_headers_and_body() {
    let mut headers = HeaderMap::new();
    headers.append(
        SET_COOKIE,
        HeaderValue::from_str(&format!("sessionKey={SECRET}; Path=/")).unwrap(),
    );
    let r = HttpResponse {
        status: StatusCode::OK,
        headers,
        body: format!(r#"{{"token":"{SECRET}"}}"#).into_bytes(),
    };
    assert_clean(&format!("{r:?}"));
}
