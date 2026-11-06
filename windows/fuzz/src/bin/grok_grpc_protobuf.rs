//! Invariants: the protobuf walk and the product scan never panic or overflow
//! the stack; recorded paths are at most five fields; `parse_usage` is `Ok`
//! with used in 0..=100, unique products in (0.05, 100] sorted by display
//! order, or an `Err` that is a bad-response custom error.
#![no_main]

use std::collections::HashSet;

use libfuzzer_sys::fuzz_target;
use tokenmon_core::{ProviderError, UsageError};
use tokenmon_provider_grok::grpc::{
    MAX_SCAN_PATH_LEN, parse_usage, read_varint, scan_for_field7_products, scan_protobuf,
};
use tokenmon_provider_grok::models::ProductCatalog;

fuzz_target!(|data: &[u8]| {
    let scan = scan_protobuf(data, 0);
    assert!(scan.fixed32.iter().all(|f| f.path.len() <= MAX_SCAN_PATH_LEN));
    assert!(scan.varints.iter().all(|v| v.path.len() <= MAX_SCAN_PATH_LEN));

    let mut index = 0;
    let end = data.len() / 2;
    let _ = read_varint(data, &mut index, end);
    assert!(index <= end);

    // The product scan over a sub-range must never read outside it.
    let _ = scan_for_field7_products(data, end / 2, end, 0);

    match parse_usage(data, chrono::DateTime::UNIX_EPOCH) {
        Ok(parsed) => {
            assert!((0.0..=100.0).contains(&parsed.used_percent));
            let mut seen = HashSet::new();
            let mut last = 0;
            for p in &parsed.products {
                assert!(seen.insert(p.id.as_str()));
                assert!(p.percent_of_pool > 0.05 && p.percent_of_pool <= 100.0);
                let index = ProductCatalog::display_index(&p.id);
                assert!(index >= last);
                last = index;
            }
        }
        Err(ProviderError::Custom {
            usage: UsageError::BadResponse(_),
            ..
        }) => {}
        Err(other) => panic!("unexpected error kind: {other:?}"),
    }
});
