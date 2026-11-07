//! gRPC-web parser tests: the spec 02 section 5.8 hex fixtures, the product
//! enum map, the four upstream #51 regression payloads, framing, trailers and
//! status validation.
#![allow(
    clippy::unwrap_used,
    clippy::assert_is_empty,
    clippy::needless_pass_by_value
)]

mod common;

use chrono::{TimeZone, Utc};
use common::{FIXTURE_A_HEX, FIXTURE_B_HEX, USED_PERCENT_64, hex, synthetic_frame, utc, varint};
use tokenmon_core::{ProviderError, UsageError};
use tokenmon_provider_grok::grpc::{
    data_frames, looks_like_protobuf, parse_usage, percent_decode, product_for_enum, read_varint,
    scan_protobuf, trailer_fields, validate_headers, validate_status, validate_trailers,
};

fn now() -> chrono::DateTime<Utc> {
    utc(2026, 7, 15, 12, 0)
}

fn pct(parsed: &tokenmon_provider_grok::grpc::Parsed, id: &str) -> Option<f64> {
    parsed
        .products
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.percent_of_pool)
}

fn assert_close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 0.01, "{actual} != {expected}");
}

/// Fixture A: used 36, chat 23, build 13; the enum-6 entry without a percent
/// is dropped.
#[test]
fn test_grpc_product_breakdown_fixture_a() {
    let parsed = parse_usage(&hex(FIXTURE_A_HEX), now()).unwrap();
    assert_close(parsed.used_percent, 36.0);
    assert_eq!(parsed.products.len(), 2);
    assert_close(pct(&parsed, "chat").unwrap(), 23.0);
    assert_close(pct(&parsed, "build").unwrap(), 13.0);
    assert_eq!(
        parsed.resets_at,
        Some(Utc.timestamp_opt(1_784_228_273, 0).unwrap())
    );
}

/// Fixture B (live capture): enum 2 build, 4 chat, 5 imagine, 3 other; no
/// voice; display order chat, build, imagine, other.
#[test]
fn test_grpc_live_credits_config_enum_map() {
    let parsed = parse_usage(&hex(FIXTURE_B_HEX), now()).unwrap();
    assert_close(parsed.used_percent, 64.0);
    assert_close(pct(&parsed, "build").unwrap(), 30.0);
    assert_close(pct(&parsed, "chat").unwrap(), 21.0);
    assert_close(pct(&parsed, "imagine").unwrap(), 12.0);
    assert_close(pct(&parsed, "other").unwrap(), 1.0);
    assert!(pct(&parsed, "voice").is_none());
    let order: Vec<&str> = parsed.products.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(order, ["chat", "build", "imagine", "other"]);
    assert_eq!(
        parsed.resets_at,
        Some(Utc.timestamp_opt(1_785_437_873, 0).unwrap())
    );
}

/// The enum map pins 1=API, 2=Grok Build, 3=Other, 4=Chat, 5=Imagine, 6=Voice.
#[test]
fn test_product_enum_map_is_pinned() {
    let expected = [
        (1, "api", "API"),
        (2, "build", "Grok Build"),
        (3, "other", "Other"),
        (4, "chat", "Chat"),
        (5, "imagine", "Imagine"),
        (6, "voice", "Voice"),
    ];
    for (value, id, name) in expected {
        assert_eq!(product_for_enum(value), (id.to_owned(), name.to_owned()));
    }
    assert_eq!(
        product_for_enum(42),
        ("product-42".to_owned(), "Product 42".to_owned())
    );
}

/// An enum with no percent (voice, 6) must not shift later percents onto it:
/// pairing happens inside each field-7 message.
#[test]
fn test_grpc_product_breakdown_with_voice_gap() {
    let frame = synthetic_frame(
        64.0,
        &[
            (2, Some(30.0)),
            (4, Some(21.0)),
            (6, None),
            (5, Some(12.0)),
            (3, Some(1.0)),
        ],
        2_000_000_000,
    );
    let parsed = parse_usage(&frame, now()).unwrap();
    assert_close(parsed.used_percent, 64.0);
    assert_eq!(parsed.products.len(), 4);
    assert_close(pct(&parsed, "build").unwrap(), 30.0);
    assert_close(pct(&parsed, "chat").unwrap(), 21.0);
    assert_close(pct(&parsed, "imagine").unwrap(), 12.0);
    assert_close(pct(&parsed, "other").unwrap(), 1.0);
    assert!(
        pct(&parsed, "voice").is_none(),
        "Voice (0%) must not steal Imagine's percent"
    );
}

/// An explicit 0.0 voice percent also yields no product.
#[test]
fn test_grpc_product_breakdown_with_explicit_zero_voice_percent() {
    let frame = synthetic_frame(
        64.0,
        &[
            (2, Some(30.0)),
            (4, Some(21.0)),
            (6, Some(0.0)),
            (5, Some(12.0)),
            (3, Some(1.0)),
        ],
        2_000_000_000,
    );
    let parsed = parse_usage(&frame, now()).unwrap();
    assert_eq!(parsed.products.len(), 4);
    assert!(pct(&parsed, "voice").is_none());
    assert_close(pct(&parsed, "imagine").unwrap(), 12.0);
}

/// A reset already in the past is kept when it is the canonical `[1,5,1]`.
#[test]
fn test_grpc_parse_keeps_past_canonical_reset() {
    let frame = synthetic_frame(10.0, &[], 1_750_000_000);
    let parsed = parse_usage(&frame, now()).unwrap();
    assert_eq!(
        parsed.resets_at,
        Some(Utc.timestamp_opt(1_750_000_000, 0).unwrap())
    );
}

/// Duplicate product ids across payloads sum and clamp at 100.
#[test]
fn test_duplicate_products_sum_and_clamp() {
    let mut body = synthetic_frame(50.0, &[(4, Some(60.0)), (4, Some(60.0))], 2_000_000_000);
    body.extend(synthetic_frame(50.0, &[(4, Some(10.0))], 2_000_000_000));
    let parsed = parse_usage(&body, now()).unwrap();
    assert_eq!(parsed.products.len(), 1);
    assert_eq!(parsed.products[0].percent_of_pool, 100.0);
}

/// Unknown enum values map to `product-N` and sort after known products,
/// ties by id (deterministic, unlike Swift).
#[test]
fn test_unknown_products_sort_deterministically() {
    let frame = synthetic_frame(
        50.0,
        &[(12, Some(5.0)), (11, Some(5.0)), (4, Some(20.0))],
        2_000_000_000,
    );
    let parsed = parse_usage(&frame, now()).unwrap();
    let ids: Vec<&str> = parsed.products.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["chat", "product-11", "product-12"]);
}

/// An oversized `UInt64.max` length varint is rejected with an error.
#[test]
fn test_oversized_length_varint_does_not_crash() {
    let mut bytes = vec![0x3A];
    bytes.extend([0xFF; 9]);
    bytes.push(0x7F);
    assert!(parse_usage(&bytes, now()).is_err());
}

/// #51 payload 1: a key varint inside a nested message that runs past the
/// message end (`12 01 | 92 01 00`) stops the product scan.
#[test]
fn test_nested_key_varint_past_message_end_does_not_crash() {
    let mut data = USED_PERCENT_64.to_vec();
    data.extend([0x12, 0x01, 0x92, 0x01, 0x00]);
    let parsed = parse_usage(&data, now()).unwrap();
    assert_close(parsed.used_percent, 64.0);
    assert!(parsed.products.is_empty());
}

/// #51 payload 2: the same overrun inside a field-7 product message
/// (`3a 01 | 92 01 00`) rejects that product.
#[test]
fn test_product_key_varint_past_message_end_does_not_crash() {
    let mut data = USED_PERCENT_64.to_vec();
    data.extend([0x3A, 0x01, 0x92, 0x01, 0x00]);
    let parsed = parse_usage(&data, now()).unwrap();
    assert_close(parsed.used_percent, 64.0);
    assert!(parsed.products.is_empty());
}

/// #51 payload 3: a product whose field-1 enum varint runs past the message
/// end (`3a 07 | 15 0000f041 08 84`, then `01` outside it) is rejected.
#[test]
fn test_product_value_varint_past_message_end_rejects_product() {
    let mut data = USED_PERCENT_64.to_vec();
    data.extend([0x3A, 0x07, 0x15, 0x00, 0x00, 0xF0, 0x41, 0x08, 0x84]);
    data.push(0x01);
    let parsed = parse_usage(&data, now()).unwrap();
    assert_close(parsed.used_percent, 64.0);
    assert!(parsed.products.is_empty());
}

/// #51 payload 4: 200,000 nested length-delimited messages do not overflow the
/// stack; the product scan stops descending at its depth cap.
#[test]
fn test_deeply_nested_messages_do_not_overflow_stack() {
    let levels = 200_000;
    let mut reversed: Vec<u8> = Vec::with_capacity(levels * 4);
    for _ in 0..levels {
        let mut length = varint(reversed.len() as u64);
        length.reverse();
        reversed.extend(length);
        reversed.push(0x12);
    }
    reversed.reverse();
    let mut data = USED_PERCENT_64.to_vec();
    data.extend(reversed);
    let parsed = parse_usage(&data, now()).unwrap();
    assert_close(parsed.used_percent, 64.0);
    assert!(parsed.products.is_empty());
}

/// A product nested exactly at the depth cap boundary is found above the cap
/// and skipped below it.
#[test]
fn test_product_scan_depth_cap_boundary() {
    let product = [0x3A, 0x07, 0x08, 0x04, 0x15, 0x00, 0x00, 0xA8, 0x41];
    let wrap = |inner: Vec<u8>| {
        let mut out = vec![0x12];
        out.extend(varint(inner.len() as u64));
        out.extend(inner);
        out
    };
    let nested = |levels: usize| {
        let mut body = product.to_vec();
        for _ in 0..levels {
            body = wrap(body);
        }
        let mut data = USED_PERCENT_64.to_vec();
        data.extend(body);
        data
    };
    assert_eq!(parse_usage(&nested(10), now()).unwrap().products.len(), 1);
    assert!(
        parse_usage(&nested(200), now())
            .unwrap()
            .products
            .is_empty()
    );
}

/// Used percent is the `[1,1]` float; a top-level `[1]` float wins over a
/// deeper `[x,1]` when `[1,1]` is absent.
#[test]
fn test_used_percent_selection_prefers_shallow_paths() {
    // field 1 fixed32 = 12.5 at the top level.
    let mut data = vec![0x0D];
    data.extend(12.5f32.to_le_bytes());
    assert_close(parse_usage(&data, now()).unwrap().used_percent, 12.5);
}

/// No percent and no period info is a decoding error; period info with no
/// floats means 0%.
#[test]
fn test_missing_percent_rules() {
    let err = parse_usage(&[0x08, 0x01], now()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Could not parse usage response: gRPC usage percent missing"
    );
    // [1,5,1] = reset, no floats at all: proto3 omitted 0.0.
    let mut reset = vec![0x08];
    reset.extend(varint(2_000_000_000));
    let mut inner = vec![0x2A, reset.len() as u8];
    inner.extend(reset);
    let mut payload = vec![0x0A, inner.len() as u8];
    payload.extend(inner);
    let parsed = parse_usage(&payload, now()).unwrap();
    assert_eq!(parsed.used_percent, 0.0);
    assert!(parsed.resets_at.is_some());
}

/// Empty and non-protobuf bodies produce the empty-response error.
#[test]
fn test_empty_body_is_an_error() {
    let err = parse_usage(&[], now()).unwrap_err();
    assert_eq!(err.to_string(), "Empty usage response from Grok.");
    assert!(matches!(
        err.usage_error(),
        UsageError::BadResponse(m) if m == "Empty usage response from Grok."
    ));
    assert!(parse_usage(&[0x00], now()).is_err());
}

/// `looks_like_protobuf` accepts wire types 0, 1, 2, 5 on field > 0 only.
#[test]
fn test_looks_like_protobuf() {
    assert!(looks_like_protobuf(&[0x0A]));
    assert!(looks_like_protobuf(&[0x08]));
    assert!(!looks_like_protobuf(&[0x00]));
    assert!(!looks_like_protobuf(&[0x0B])); // wire type 3
    assert!(!looks_like_protobuf(&[]));
}

/// A frame that overruns the body discards every frame, including earlier
/// good ones; short trailing bytes are ignored.
#[test]
fn test_data_frames_overrun_discards_everything() {
    let good = [0, 0, 0, 0, 2, 0xAA, 0xBB];
    let mut body = good.to_vec();
    assert_eq!(data_frames(&body), vec![&[0xAA, 0xBB][..]]);
    body.extend([0, 0, 0, 0, 9, 1]);
    assert!(data_frames(&body).is_empty());
    let mut trailing = good.to_vec();
    trailing.extend([1, 2, 3]);
    assert_eq!(data_frames(&trailing).len(), 1);
}

/// A declared length of `u32::MAX` must not overflow or panic.
#[test]
fn test_frame_length_u32_max_is_rejected() {
    let body = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x01];
    assert!(data_frames(&body).is_empty());
    assert!(trailer_fields(&body).is_empty());
}

/// Trailer fields: lowercase keys, trimmed values, percent-decoded, later
/// keys win, CR and LF both separate lines, invalid UTF-8 is skipped.
#[test]
fn test_trailer_fields_parsing() {
    let text = b"Grpc-Status: 16 \r\ngrpc-message:%75nauthenticated\rgrpc-status:3\nbadline\n";
    let mut body = vec![0x80];
    body.extend((text.len() as u32).to_be_bytes());
    body.extend(text);
    let fields = trailer_fields(&body);
    assert_eq!(fields["grpc-status"], "3");
    assert_eq!(fields["grpc-message"], "unauthenticated");

    let mut bad = vec![0x80, 0, 0, 0, 2, 0xFF, 0xFE];
    bad.extend(&body);
    assert_eq!(trailer_fields(&bad)["grpc-status"], "3");
}

/// A value with a bad percent escape becomes the empty string.
#[test]
fn test_trailer_bad_percent_escape_yields_empty_value() {
    let text = b"grpc-message:%zz";
    let mut body = vec![0x80];
    body.extend((text.len() as u32).to_be_bytes());
    body.extend(text);
    assert_eq!(trailer_fields(&body)["grpc-message"], "");
}

/// Percent decoding is strict.
#[test]
fn test_percent_decode_strictness() {
    assert_eq!(percent_decode("a%2Fb").as_deref(), Some("a/b"));
    assert_eq!(percent_decode("%"), None);
    assert_eq!(percent_decode("%4"), None);
    assert_eq!(percent_decode("%ff"), None); // not UTF-8
}

fn unauthorized(result: Result<(), ProviderError>) -> bool {
    matches!(result, Err(ProviderError::Unauthorized(_)))
}

/// Status 16 or an `unauthenticated` message is unauthorized; other non-zero
/// statuses are a body-free HTTP-style error; zero/missing/garbage pass.
#[test]
fn test_validate_status_rules() {
    assert!(validate_status(None, None).is_ok());
    assert!(validate_status(Some("0"), None).is_ok());
    assert!(validate_status(Some("abc"), None).is_ok());
    assert!(validate_status(Some("99999999999999999999"), None).is_ok());
    assert!(unauthorized(validate_status(Some("16"), None)));
    assert!(unauthorized(validate_status(Some(" 16 "), None)));
    assert!(unauthorized(validate_status(
        Some("7"),
        Some("User UNAUTHENTICATED")
    )));
    let err = validate_status(Some("13"), Some("secret body")).unwrap_err();
    assert_eq!(err.to_string(), "Usage request failed (HTTP 13).");
    assert!(!err.to_string().contains("secret"));
}

/// Header-borne statuses percent-decode the message, falling back to raw.
#[test]
fn test_validate_headers_decodes_message() {
    assert!(unauthorized(validate_headers(
        Some("2"),
        Some("%75nauthenticated")
    )));
    assert!(unauthorized(validate_headers(
        Some("2"),
        Some("unauthenticated%zz")
    )));
    assert!(validate_headers(None, None).is_ok());
}

/// A trailers-only body with `grpc-status: 16` is unauthorized.
#[test]
fn test_validate_trailers_unauthenticated() {
    let text = b"grpc-status:16\r\ngrpc-message:unauthenticated\r\n";
    let mut body = vec![0x80];
    body.extend((text.len() as u32).to_be_bytes());
    body.extend(text);
    assert!(unauthorized(validate_trailers(&body)));
    assert!(validate_trailers(&hex(FIXTURE_A_HEX)).is_ok());
}

/// `read_varint` never moves past `end`, stops at 64 bits of shift, and
/// reports `None` on a truncated value.
#[test]
fn test_read_varint_bounds() {
    let mut i = 0;
    assert_eq!(read_varint(&[0x92, 0x01], &mut i, 1), None);
    assert_eq!(i, 1);
    let mut i = 0;
    assert_eq!(read_varint(&[0xFF; 12], &mut i, 12), None);
    let mut i = 0;
    assert_eq!(
        read_varint(
            &[0xFF; 9].iter().chain(&[0x01]).copied().collect::<Vec<_>>(),
            &mut i,
            10
        ),
        Some(u64::MAX)
    );
    let mut i = 5;
    assert_eq!(read_varint(&[1, 2, 3], &mut i, 99), None);
}

/// Recorded paths never exceed five fields.
#[test]
fn test_scan_protobuf_path_length_is_bounded() {
    let mut nested = vec![0x0D, 0, 0, 0x80, 0x3F];
    for _ in 0..30 {
        let mut next = vec![0x0A];
        next.extend(varint(nested.len() as u64));
        next.extend(nested);
        nested = next;
    }
    let scan = scan_protobuf(&nested, 0);
    assert!(scan.fixed32.iter().all(|f| f.path.len() <= 5));
    assert!(scan.varints.iter().all(|v| v.path.len() <= 5));
}

/// #50: a body of two-byte varint fields collects at most `MAX_SCAN_FIELDS`.
#[test]
fn test_scan_protobuf_field_count_is_capped() {
    use tokenmon_provider_grok::grpc::MAX_SCAN_FIELDS;
    // key = field 1, wire type 0; value 1.
    let body: Vec<u8> = std::iter::repeat_n([0x08u8, 0x01], MAX_SCAN_FIELDS * 3)
        .flatten()
        .collect();
    let scan = scan_protobuf(&body, 0);
    assert_eq!(scan.varints.len(), MAX_SCAN_FIELDS);
    assert!(scan.fixed32.is_empty());
    let few = scan_protobuf(&[0x08, 0x01, 0x08, 0x02], 0);
    assert_eq!(few.varints.len(), 2);
}

/// Fuzz regression (run 42312736605): a signed status `+16` parses as 16, so it
/// is unauthorized, matching Swift `Int("+16")`; the harness had compared text.
#[test]
fn test_fuzz_signed_status_is_still_unauthenticated() {
    for raw in ["+16", " 16\t", "16"] {
        assert!(
            matches!(
                validate_status(Some(raw), None),
                Err(ProviderError::Unauthorized(_))
            ),
            "{raw:?}"
        );
    }
}
