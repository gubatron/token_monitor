#![no_main]
//! Fuzzes the JSON number/decimal helpers on arbitrary JSON and on arbitrary
//! strings. Must never panic; `number` is always finite and `decimal` implies `number`.

use libfuzzer_sys::fuzz_target;
use serde_json::Value;
use tokenmon_core::util::json::{decimal, first_bool, first_double, nested, number};

fn check(value: &Value) {
    let n = number(Some(value));
    if let Some(n) = n {
        assert!(n.is_finite());
    }
    if decimal(Some(value)).is_some() {
        assert!(n.is_some());
    }
    if let Some(obj) = value.as_object() {
        let _ = first_double(obj, &["val", "value", "a"]);
        let _ = first_bool(obj, &["flag", "val"], false);
        let _ = nested(obj, &["a", "b", "c"]);
        for child in obj.values() {
            check(child);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = serde_json::from_slice::<Value>(data) {
        check(&value);
    }
    check(&Value::String(String::from_utf8_lossy(data).into_owned()));
});
