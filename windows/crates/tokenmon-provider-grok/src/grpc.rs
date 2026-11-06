//! gRPC-web `GetGrokCreditsConfig` response parser (spec 02 sections 1.2.2 to
//! 1.2.5).
//!
//! The body is untrusted bytes. Every read is bounded to the end of the
//! message being walked, offsets use checked arithmetic, and the recursive
//! product scan stops at [`MAX_PRODUCT_SCAN_DEPTH`]. These are the fixes the
//! upstream Swift parser received in faulknerpearce/token_monitor#51 (see
//! spec 02 section 4, divergences 1 and 2); none of the functions here can
//! panic or overflow the stack.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use tokenmon_core::{ProviderError, UsageError};

use crate::models::{ProductCatalog, ProductUsage, clamp_percent};

/// Deepest nesting the product scan descends into. The live response nests
/// products two levels down; the cap stops a hostile body of nested
/// length-delimited fields from overflowing the stack.
pub const MAX_PRODUCT_SCAN_DEPTH: usize = 64;

/// Depth (0-based) at which the generic scan stops recursing; recorded paths
/// are therefore at most five fields long.
pub const MAX_SCAN_DEPTH: usize = 4;

/// Longest path [`scan_protobuf`] can record.
pub const MAX_SCAN_PATH_LEN: usize = MAX_SCAN_DEPTH + 1;

/// Smallest value treated as a plausible Unix reset timestamp.
const RESET_MIN_SECS: u64 = 1_700_000_000;
/// Largest value treated as a plausible Unix reset timestamp.
const RESET_MAX_SECS: u64 = 2_100_000_000;

/// Decoded usage percent, reset instant and product slices of one response.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// Total used share of the pool, `0.0..=100.0`.
    pub used_percent: f64,
    /// Reset instant, when present.
    pub resets_at: Option<DateTime<Utc>>,
    /// Products in display order; ids are unique.
    pub products: Vec<ProductUsage>,
}

/// A fixed32 (float) field found by [`scan_protobuf`].
#[derive(Debug, Clone, PartialEq)]
pub struct Fixed32Field {
    /// Field numbers from the root to the field.
    pub path: Vec<u64>,
    /// The little-endian IEEE-754 value.
    pub value: f32,
    /// Position in scan order across all payloads.
    pub order: usize,
}

/// A varint field found by [`scan_protobuf`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarintField {
    /// Field numbers from the root to the field.
    pub path: Vec<u64>,
    /// The decoded value.
    pub value: u64,
}

/// Result of the schema-less protobuf walk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    /// Every fixed32 field in scan order.
    pub fixed32: Vec<Fixed32Field>,
    /// Every varint field in scan order.
    pub varints: Vec<VarintField>,
    /// The next fixed32 order number.
    pub order: usize,
}

fn custom_bad_response(message: String) -> ProviderError {
    ProviderError::Custom {
        usage: UsageError::BadResponse(message.clone()),
        message,
    }
}

/// `grokHTTPStatus`: a body-free "Usage request failed (HTTP n)." error.
pub fn http_status_error(code: i64) -> ProviderError {
    custom_bad_response(format!("Usage request failed (HTTP {code})."))
}

/// `grokDecoding`: "Could not parse usage response: detail".
pub fn decoding_error(detail: &str) -> ProviderError {
    custom_bad_response(format!("Could not parse usage response: {detail}"))
}

/// `grokEmptyResponse`.
pub fn empty_response_error() -> ProviderError {
    custom_bad_response("Empty usage response from Grok.".to_owned())
}

/// Maps an enum value of field 1 inside a field-7 message to `(id, name)`.
///
/// Verified against a live SuperGrok response (Build 30%, Chat 21%, Imagine
/// 12%, Other 1%). Unknown values become `product-N` / `Product N`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::grpc::product_for_enum;
/// assert_eq!(product_for_enum(5), ("imagine".to_owned(), "Imagine".to_owned()));
/// assert_eq!(product_for_enum(9).0, "product-9");
/// ```
pub fn product_for_enum(value: u64) -> (String, String) {
    let (id, name) = match value {
        1 => ("api", "API"),
        2 => ("build", "Grok Build"),
        3 => ("other", "Other"),
        4 => ("chat", "Chat"),
        5 => ("imagine", "Imagine"),
        6 => ("voice", "Voice"),
        n => return (format!("product-{n}"), format!("Product {n}")),
    };
    (id.to_owned(), name.to_owned())
}

/// Reads a LEB128 varint from `bytes[*index..end]`.
///
/// Returns `None` when the input ends (at `end` or at the buffer end) before a
/// terminating byte, or when the value needs more than 64 bits of shift.
/// `*index` never moves past `min(end, bytes.len())`.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::grpc::read_varint;
/// let mut i = 0;
/// assert_eq!(read_varint(&[0xAC, 0x02], &mut i, 2), Some(300));
/// let mut i = 0;
/// assert_eq!(read_varint(&[0x92, 0x01], &mut i, 1), None); // runs past `end`
/// ```
pub fn read_varint(bytes: &[u8], index: &mut usize, end: usize) -> Option<u64> {
    let limit = end.min(bytes.len());
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    while *index < limit && shift < 64 {
        let byte = bytes[*index];
        *index += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

/// `index + n <= end`, with checked arithmetic.
fn fits(index: usize, n: usize, end: usize) -> bool {
    index.checked_add(n).is_some_and(|stop| stop <= end)
}

fn read_f32_le(bytes: &[u8], index: usize) -> Option<f32> {
    let chunk: [u8; 4] = bytes.get(index..index.checked_add(4)?)?.try_into().ok()?;
    Some(f32::from_le_bytes(chunk))
}

/// Splits a gRPC-web body into data-frame payloads (`flags & 0x80 == 0`).
///
/// A frame that overruns the body discards **all** frames, including earlier
/// good ones. Trailing bytes shorter than a frame header are ignored.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::grpc::data_frames;
/// let body = [0, 0, 0, 0, 2, 0xAA, 0xBB, 0x80, 0, 0, 0, 1, 0xCC];
/// assert_eq!(data_frames(&body), vec![&[0xAA, 0xBB][..]]);
/// assert!(data_frames(&[0, 0, 0, 0, 9, 1]).is_empty());
/// ```
pub fn data_frames(data: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut index = 0usize;
    while fits(index, 5, data.len()) {
        let Some((flags, payload, next)) = frame_at(data, index) else {
            return Vec::new();
        };
        if flags & 0x80 == 0 {
            frames.push(payload);
        }
        index = next;
    }
    frames
}

/// The frame at `index` (header already known to fit): flags, payload and the
/// index after it; `None` when the declared length overruns `data`.
fn frame_at(data: &[u8], index: usize) -> Option<(u8, &[u8], usize)> {
    let flags = *data.get(index)?;
    let len_bytes: [u8; 4] = data.get(index + 1..index + 5)?.try_into().ok()?;
    let length = usize::try_from(u32::from_be_bytes(len_bytes)).ok()?;
    let start = index.checked_add(5)?;
    let end = start.checked_add(length)?;
    Some((flags, data.get(start..end)?, end))
}

/// Characters Foundation's `.newlines` splits on.
fn is_newline(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Strict percent-decoding like Foundation's `removingPercentEncoding`:
/// `None` on a malformed escape or when the result is not UTF-8.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::grpc::percent_decode;
/// assert_eq!(percent_decode("a%20b").as_deref(), Some("a b"));
/// assert_eq!(percent_decode("%zz"), None);
/// ```
pub fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = char::from(hex[0]).to_digit(16)?;
            let lo = char::from(hex[1]).to_digit(16)?;
            out.push(u8::try_from(hi * 16 + lo).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Collects `key: value` lines of trailer frames (`flags & 0x80 != 0`).
///
/// Keys are trimmed and lowercased; values are trimmed and percent-decoded
/// (`""` when decoding fails). On an overrun the fields collected so far are
/// kept. Later keys overwrite earlier ones.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::grpc::trailer_fields;
/// let mut body = vec![0x80, 0, 0, 0, 15];
/// body.extend_from_slice(b"grpc-status:0\r\n");
/// assert_eq!(trailer_fields(&body)["grpc-status"], "0");
/// ```
pub fn trailer_fields(data: &[u8]) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let mut index = 0usize;
    while fits(index, 5, data.len()) {
        let Some((flags, payload, next)) = frame_at(data, index) else {
            break;
        };
        if flags & 0x80 != 0
            && let Ok(text) = std::str::from_utf8(payload)
        {
            collect_trailer_lines(text, &mut fields);
        }
        index = next;
    }
    fields
}

fn collect_trailer_lines(text: &str, fields: &mut HashMap<String, String>) {
    for line in text.split(is_newline).filter(|l| !l.is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = percent_decode(value.trim()).unwrap_or_default();
        fields.insert(key.trim().to_lowercase(), value);
    }
}

/// Applies the `grpc-status` rules of a trailer or trailers-only reply.
///
/// Nothing happens for a missing, non-integer or zero status. Status 16 or a
/// message containing `unauthenticated` is [`ProviderError::Unauthorized`];
/// any other status is a body-free "Usage request failed (HTTP n)." error.
///
/// # Examples
///
/// ```
/// use tokenmon_provider_grok::grpc::validate_status;
/// assert!(validate_status(Some("0"), None).is_ok());
/// assert!(validate_status(Some("16"), None).is_err());
/// ```
pub fn validate_status(raw: Option<&str>, message: Option<&str>) -> Result<(), ProviderError> {
    let Some(raw) = raw else { return Ok(()) };
    let Ok(status) = raw.trim_matches([' ', '\t']).parse::<i64>() else {
        return Ok(());
    };
    if status == 0 {
        return Ok(());
    }
    let message = message.unwrap_or_default();
    if status == 16 || message.to_lowercase().contains("unauthenticated") {
        return Err(ProviderError::Unauthorized(
            tokenmon_core::ProviderErrorContext::Grok,
        ));
    }
    Err(http_status_error(status))
}

/// Applies [`validate_status`] to the trailer frames of a body.
pub fn validate_trailers(data: &[u8]) -> Result<(), ProviderError> {
    let fields = trailer_fields(data);
    validate_status(
        fields.get("grpc-status").map(String::as_str),
        fields.get("grpc-message").map(String::as_str),
    )
}

/// Applies [`validate_status`] to `grpc-status` / `grpc-message` HTTP headers
/// (a trailers-only reply). The message is percent-decoded when possible.
pub fn validate_headers(status: Option<&str>, message: Option<&str>) -> Result<(), ProviderError> {
    let decoded = message.map(|m| percent_decode(m).unwrap_or_else(|| m.to_owned()));
    validate_status(status, decoded.as_deref())
}

/// Whether the first byte is a plausible protobuf key (field > 0, wire type
/// 0, 1, 2 or 5).
pub fn looks_like_protobuf(data: &[u8]) -> bool {
    data.first()
        .is_some_and(|b| b >> 3 > 0 && matches!(b & 7, 0 | 1 | 2 | 5))
}

/// Schema-less protobuf walk of one payload (spec 1.2.3).
///
/// Records fixed32 and varint fields with their field-number paths. Every
/// length-delimited field is also scanned as a message down to depth 4, so
/// strings can produce garbage entries; consumers select by exact path. Any
/// malformed field ends the walk and returns what was collected.
pub fn scan_protobuf(data: &[u8], order: usize) -> Scan {
    let mut scan = Scan {
        order,
        ..Scan::default()
    };
    scan_into(data, 0, &mut Vec::new(), &mut scan);
    scan
}

fn scan_into(bytes: &[u8], depth: usize, path: &mut Vec<u64>, scan: &mut Scan) {
    let end = bytes.len();
    let mut index = 0usize;
    while index < end {
        let Some(key) = read_varint(bytes, &mut index, end).filter(|k| *k != 0) else {
            return;
        };
        path.push(key >> 3);
        let keep_going = scan_field(bytes, key & 7, &mut index, depth, path, scan);
        path.pop();
        if !keep_going {
            return;
        }
    }
}

/// Handles one field's value; `false` ends the walk of this message.
fn scan_field(
    bytes: &[u8],
    wire: u64,
    index: &mut usize,
    depth: usize,
    path: &mut Vec<u64>,
    scan: &mut Scan,
) -> bool {
    let end = bytes.len();
    match wire {
        0 => {
            let Some(value) = read_varint(bytes, index, end) else {
                return false;
            };
            scan.varints.push(VarintField {
                path: path.clone(),
                value,
            });
            true
        }
        1 => {
            if !fits(*index, 8, end) {
                return false;
            }
            *index += 8;
            true
        }
        2 => {
            let Some(len) = read_varint(bytes, index, end) else {
                return false;
            };
            let Some(sub_end) = usize::try_from(len)
                .ok()
                .filter(|l| fits(*index, *l, end))
                .map(|l| *index + l)
            else {
                return false;
            };
            if depth < MAX_SCAN_DEPTH {
                scan_into(&bytes[*index..sub_end], depth + 1, path, scan);
            }
            *index = sub_end;
            true
        }
        5 => {
            let Some(value) = read_f32_le(bytes, *index).filter(|_| fits(*index, 4, end)) else {
                return false;
            };
            scan.fixed32.push(Fixed32Field {
                path: path.clone(),
                value,
                order: scan.order,
            });
            scan.order += 1;
            *index += 4;
            true
        }
        _ => false,
    }
}

/// Walks `bytes[start..end]`, treating the body of every length-delimited
/// field 7 as a product message and recursing into every other
/// length-delimited field. Pairing enum and percent inside each field-7
/// message is required: zipping them flat misassigns Imagine's 12% to Voice.
///
/// Every read stays within `end`; a field that would overrun stops the walk
/// with the products found so far. Fields nested deeper than
/// [`MAX_PRODUCT_SCAN_DEPTH`] are skipped, not scanned.
pub fn scan_for_field7_products(
    bytes: &[u8],
    start: usize,
    end: usize,
    depth: usize,
) -> Vec<ProductUsage> {
    let end = end.min(bytes.len());
    let mut products = Vec::new();
    let mut index = start;
    while index < end {
        let Some(key) = read_varint(bytes, &mut index, end).filter(|k| *k != 0) else {
            break;
        };
        let (field, wire) = (key >> 3, key & 7);
        let advanced = match wire {
            0 => read_varint(bytes, &mut index, end).map(|_| ()),
            1 => skip(&mut index, 8, end),
            5 => skip(&mut index, 4, end),
            2 => product_scan_delimited(bytes, &mut index, end, field, depth, &mut products),
            _ => None,
        };
        if advanced.is_none() {
            return products;
        }
    }
    products
}

fn skip(index: &mut usize, n: usize, end: usize) -> Option<()> {
    if fits(*index, n, end) {
        *index += n;
        Some(())
    } else {
        None
    }
}

fn product_scan_delimited(
    bytes: &[u8],
    index: &mut usize,
    end: usize,
    field: u64,
    depth: usize,
    products: &mut Vec<ProductUsage>,
) -> Option<()> {
    let len = usize::try_from(read_varint(bytes, index, end)?).ok()?;
    if !fits(*index, len, end) {
        return None;
    }
    let (sub_start, sub_end) = (*index, *index + len);
    if field == 7 {
        products.extend(parse_product_sub_message(bytes, sub_start, sub_end));
    } else if depth < MAX_PRODUCT_SCAN_DEPTH {
        products.extend(scan_for_field7_products(
            bytes,
            sub_start,
            sub_end,
            depth + 1,
        ));
    }
    *index = sub_end;
    Some(())
}

/// Decodes one product from `bytes[start..end]`; `None` when a field would
/// overrun `end`, the wire type is unknown, or the enum or percent is missing.
/// A percent must satisfy `0.05 < percent <= 100`.
pub fn parse_product_sub_message(bytes: &[u8], start: usize, end: usize) -> Option<ProductUsage> {
    let end = end.min(bytes.len());
    let mut index = start;
    let mut enum_value: Option<u64> = None;
    let mut percent: Option<f64> = None;
    while index < end {
        let key = read_varint(bytes, &mut index, end).filter(|k| *k != 0)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let value = read_varint(bytes, &mut index, end)?;
                if field == 1 {
                    enum_value = Some(value);
                }
            }
            1 => skip(&mut index, 8, end)?,
            5 => {
                if !fits(index, 4, end) {
                    return None;
                }
                if field == 2 {
                    percent = read_f32_le(bytes, index).map(f64::from);
                }
                index += 4;
            }
            2 => {
                let len = usize::try_from(read_varint(bytes, &mut index, end)?).ok()?;
                skip(&mut index, len, end)?;
            }
            _ => return None,
        }
    }
    let percent = percent.filter(|p| *p > 0.05 && *p <= 100.0)?;
    let (id, name) = product_for_enum(enum_value?);
    Some(ProductUsage::new(&id, name, percent))
}

/// Merges products across payloads by lowercased id (percents add, clamped to
/// 100) and sorts them into display order, ties by id.
fn merge_products(payloads: &[&[u8]]) -> Vec<ProductUsage> {
    let mut by_id: HashMap<String, ProductUsage> = HashMap::new();
    for payload in payloads {
        for product in scan_for_field7_products(payload, 0, payload.len(), 0) {
            by_id
                .entry(product.id.to_lowercase())
                .and_modify(|existing| {
                    existing.percent_of_pool =
                        clamp_percent(existing.percent_of_pool + product.percent_of_pool);
                })
                .or_insert(product);
        }
    }
    let mut merged: Vec<ProductUsage> = by_id.into_values().collect();
    ProductCatalog::sort_for_display(&mut merged);
    merged
}

fn percent_from_fixed32(fixed32: &[Fixed32Field]) -> Option<f64> {
    let in_range = |f: &&Fixed32Field| f.value.is_finite() && (0.0..=100.0).contains(&f.value);
    fixed32
        .iter()
        .filter(in_range)
        .find(|f| f.path == [1, 1])
        .or_else(|| {
            fixed32
                .iter()
                .filter(in_range)
                .filter(|f| f.path.last() == Some(&1) && f.path.len() <= 2)
                .min_by_key(|f| (f.path.len(), f.order))
        })
        .map(|f| f64::from(f.value))
}

fn reset_from_varints(varints: &[VarintField], now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let candidates: Vec<(&[u64], DateTime<Utc>)> = varints
        .iter()
        .filter(|v| (RESET_MIN_SECS..=RESET_MAX_SECS).contains(&v.value))
        .filter_map(|v| {
            let secs = i64::try_from(v.value).ok()?;
            Some((v.path.as_slice(), DateTime::from_timestamp(secs, 0)?))
        })
        .collect();
    let canonical = candidates
        .iter()
        .filter(|(path, _)| *path == [1, 5, 1])
        .map(|(_, at)| *at)
        .min();
    // The canonical reset field is kept even when already past, so a lagging
    // payload keeps the period anchor; other candidates count only in the
    // future so unrelated old timestamps are ignored.
    canonical.or_else(|| {
        candidates
            .iter()
            .map(|(_, at)| *at)
            .filter(|at| *at > now)
            .min()
    })
}

/// Parses a gRPC-web body into used percent, reset and products.
///
/// Falls back to treating the whole body as one protobuf message when it has
/// no frames but looks like protobuf.
///
/// # Errors
///
/// An empty-response error when there is nothing to parse, and a decoding
/// error ("gRPC usage percent missing") when no used percent can be derived.
/// Both are `ProviderError::Custom` with a `BadResponse` usage error.
///
/// # Examples
///
/// ```
/// use chrono::Utc;
/// use tokenmon_provider_grok::grpc::parse_usage;
/// // 0a 05 0d <64.0f32>: path [1,1] = 64.0
/// let body = [0x0A, 0x05, 0x0D, 0x00, 0x00, 0x80, 0x42];
/// assert_eq!(parse_usage(&body, Utc::now()).unwrap().used_percent, 64.0);
/// ```
pub fn parse_usage(data: &[u8], now: DateTime<Utc>) -> Result<Parsed, ProviderError> {
    let mut payloads = data_frames(data);
    if payloads.is_empty() && looks_like_protobuf(data) {
        payloads = vec![data];
    }
    if payloads.is_empty() {
        return Err(empty_response_error());
    }

    let mut all = Scan::default();
    for payload in &payloads {
        let scan = scan_protobuf(payload, all.order);
        all.fixed32.extend(scan.fixed32);
        all.varints.extend(scan.varints);
        all.order = scan.order;
    }

    let percent = percent_from_fixed32(&all.fixed32);
    let products = merge_products(&payloads);
    let resets_at = reset_from_varints(&all.varints, now);
    let has_period = all
        .varints
        .iter()
        .any(|v| v.path.starts_with(&[1, 5]) || (v.path == [1, 8, 1] && matches!(v.value, 1 | 2)));
    let used = percent
        .or_else(|| (resets_at.is_some() && has_period && all.fixed32.is_empty()).then_some(0.0));
    let used_percent = used.ok_or_else(|| decoding_error("gRPC usage percent missing"))?;
    Ok(Parsed {
        used_percent,
        resets_at,
        products,
    })
}
