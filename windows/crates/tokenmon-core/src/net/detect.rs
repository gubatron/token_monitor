//! Bot-challenge, HTML and login-page detection (spec 01 section 1.1.3).
//!
//! All functions are total over arbitrary bytes and headers.

use reqwest::StatusCode;
use reqwest::header::HeaderMap;

const HTML_SNIFF_BYTES: usize = 512;
const LOGIN_SNIFF_BYTES: usize = 65_536;

/// Markers of a Cloudflare-style challenge page (lowercase).
const CHALLENGE_MARKERS: [&str; 6] = [
    "cf-chl",
    "challenge-platform",
    "cf_chl_opt",
    "just a moment...",
    "attention required! | cloudflare",
    "cf-browser-verification",
];

/// Markers of a sign-in page (lowercase).
const LOGIN_MARKERS: [&str; 10] = [
    "type=\"password\"",
    "type='password'",
    "sign in",
    "sign-in",
    "signin",
    "log in",
    "log-in",
    "login",
    "authkit",
    "workos",
];

/// Whether the body looks like an HTML document: the first 512 bytes, trimmed of
/// leading whitespace, start with `<`.
///
/// A body whose first 512 bytes are not valid UTF-8 is not HTML. A multi-byte
/// character merely cut by the 512-byte window does not count as invalid (the
/// Swift original returns false there; this is a deliberate divergence).
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::looks_like_html;
///
/// assert!(looks_like_html(b"  \n<!doctype html>"));
/// assert!(!looks_like_html(b"{\"a\":1}"));
/// assert!(!looks_like_html(&[0xff, b'<']));
/// ```
pub fn looks_like_html(body: &[u8]) -> bool {
    let head = &body[..body.len().min(HTML_SNIFF_BYTES)];
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        // Incomplete trailing sequence only: the window cut a character.
        Err(e) if e.error_len().is_none() && body.len() > HTML_SNIFF_BYTES => {
            std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or("")
        }
        Err(_) => return false,
    };
    text.trim_start().starts_with('<')
}

fn contains_any(haystack: &[u8], needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|n| haystack.windows(n.len()).any(|w| w == n.as_bytes()))
}

/// Whether the body is an HTML sign-in page (and not a bot challenge).
///
/// Requires [`looks_like_html`]. Scans the first 64 KiB, lowercased (ASCII case
/// folding is equivalent to the spec's Latin-1 decode because every marker is
/// ASCII). Any challenge marker makes the answer false; otherwise any login
/// marker makes it true.
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::looks_like_login_page;
///
/// assert!(looks_like_login_page(b"<html><form>Please Sign In</form>"));
/// assert!(!looks_like_login_page(b"<html>Just a moment... sign in</html>"));
/// assert!(!looks_like_login_page(b"{\"login\":true}"));
/// ```
pub fn looks_like_login_page(body: &[u8]) -> bool {
    if !looks_like_html(body) {
        return false;
    }
    let lowered = body[..body.len().min(LOGIN_SNIFF_BYTES)].to_ascii_lowercase();
    if contains_any(&lowered, &CHALLENGE_MARKERS) {
        return false;
    }
    contains_any(&lowered, &LOGIN_MARKERS)
}

/// Whether a response is a bot-protection challenge rather than a real answer:
/// it carries a `cf-mitigated` header (any value), or it is a 403 with an HTML
/// body.
///
/// # Examples
///
/// ```
/// use reqwest::{StatusCode, header::HeaderMap};
/// use tokenmon_core::net::is_bot_challenge;
///
/// let h = HeaderMap::new();
/// assert!(is_bot_challenge(StatusCode::FORBIDDEN, &h, b"<html>"));
/// assert!(!is_bot_challenge(StatusCode::FORBIDDEN, &h, b"{}"));
/// assert!(!is_bot_challenge(StatusCode::OK, &h, b"<html>"));
/// ```
pub fn is_bot_challenge(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> bool {
    headers.contains_key("cf-mitigated")
        || (status == StatusCode::FORBIDDEN && looks_like_html(body))
}

/// Whether a provider error string means "not signed in": the lowercased
/// message contains `not_authenticated` or `unauthor`.
///
/// # Examples
///
/// ```
/// use tokenmon_core::net::is_unauthorized_message;
///
/// assert!(is_unauthorized_message("NOT_AUTHENTICATED"));
/// assert!(is_unauthorized_message("Unauthorized"));
/// assert!(!is_unauthorized_message("quota exceeded"));
/// ```
pub fn is_unauthorized_message(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("not_authenticated") || m.contains("unauthor")
}
