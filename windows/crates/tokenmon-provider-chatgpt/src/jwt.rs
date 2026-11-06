//! Claims of the ChatGPT web access-token JWT (spec 03 section 2.4).
//!
//! The signature is never verified: the claims only pick a request header and a
//! cache deadline. Every function is total (no input panics).

use std::fmt;

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use tokenmon_core::util::json::number;

/// Claim object that carries the account id.
pub const AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// The well-known account id claim inside [`AUTH_CLAIM`].
pub const ACCOUNT_ID_CLAIM: &str = "chatgpt_account_id";

/// Members of the auth claim in **document order** (a duplicate key keeps its
/// first position and its last value, like a dictionary built in one pass).
#[derive(Debug, Default)]
struct OrderedClaim(Option<Vec<(String, Value)>>);

impl<'de> Deserialize<'de> for OrderedClaim {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OrderedClaim;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs: Vec<(String, Value)> = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, Value>()? {
                    match pairs.iter_mut().find(|(existing, _)| *existing == k) {
                        Some(slot) => slot.1 = v,
                        None => pairs.push((k, v)),
                    }
                }
                Ok(OrderedClaim(Some(pairs)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(OrderedClaim(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E> {
                Ok(OrderedClaim(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E> {
                Ok(OrderedClaim(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E> {
                Ok(OrderedClaim(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E> {
                Ok(OrderedClaim(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<Self::Value, E> {
                Ok(OrderedClaim(None))
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(OrderedClaim(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// The two claims we read; everything else is skipped.
#[derive(Debug, Default)]
struct Claims {
    exp: Option<Value>,
    auth: Option<Vec<(String, Value)>>,
}

impl<'de> Deserialize<'de> for Claims {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Claims;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut claims = Claims::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "exp" => claims.exp = Some(map.next_value::<Value>()?),
                        AUTH_CLAIM => claims.auth = map.next_value::<OrderedClaim>()?.0,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(claims)
            }
        }
        d.deserialize_map(V)
    }
}

/// Strict standard-alphabet base64 decoder (Foundation `Data(base64Encoded:)`
/// rules: length multiple of 4, padding only at the end, no whitespace).
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let pad = bytes.iter().rev().take_while(|b| **b == b'=').count();
    if pad > 2 {
        return None;
    }
    let body = &bytes[..bytes.len() - pad];
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in body {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `base64URLDecoded`: URL alphabet to standard, pad to a multiple of 4, strict decode.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::jwt::base64_url_decode;
/// assert_eq!(base64_url_decode("eyJhIjoxfQ").unwrap(), br#"{"a":1}"#);
/// assert!(base64_url_decode("a").is_none());
/// assert!(base64_url_decode("é").is_none());
/// ```
pub fn base64_url_decode(value: &str) -> Option<Vec<u8>> {
    let mut s: String = value
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    let remainder = s.chars().count() % 4;
    if remainder > 0 {
        s.extend(std::iter::repeat_n('=', 4 - remainder));
    }
    decode_base64(&s)
}

fn claims(token: &str) -> Option<Claims> {
    // Swift `split(separator:)` drops empty pieces, so `.a.b` has payload `b`.
    let payload = token.split('.').filter(|p| !p.is_empty()).nth(1)?;
    let bytes = base64_url_decode(payload)?;
    serde_json::from_slice::<Claims>(&bytes).ok()
}

/// `ChatGPTAccountID.fromAccessToken`: the `chatgpt_account_id` string of the
/// `https://api.openai.com/auth` claim; otherwise the first string value (in
/// document order) whose lowercased key contains `account_id`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::jwt::account_id;
/// // payload {"https://api.openai.com/auth":{"chatgpt_account_id":"acct_alt"}}
/// let t = "h.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdF9hbHQifX0.s";
/// assert_eq!(account_id(t).as_deref(), Some("acct_alt"));
/// assert_eq!(account_id("garbage"), None);
/// ```
pub fn account_id(token: &str) -> Option<String> {
    let pairs = claims(token)?.auth?;
    let exact = pairs
        .iter()
        .find(|(k, _)| k == ACCOUNT_ID_CLAIM)
        .and_then(|(_, v)| v.as_str());
    if let Some(id) = exact {
        return Some(id.to_owned());
    }
    pairs
        .iter()
        .filter(|(k, _)| k.to_lowercase().contains("account_id"))
        .find_map(|(_, v)| v.as_str().map(str::to_owned))
}

/// `ChatGPTAccountID.expiry`: the `exp` claim in epoch seconds, or `None` when
/// the token is not a readable JWT. Always finite.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_chatgpt::jwt::expiry_secs;
/// // payload {"exp":1790003600}
/// assert_eq!(expiry_secs("h.eyJleHAiOjE3OTAwMDM2MDB9.s"), Some(1_790_003_600.0));
/// assert_eq!(expiry_secs("opaque"), None);
/// ```
pub fn expiry_secs(token: &str) -> Option<f64> {
    number(claims(token)?.exp.as_ref())
}
